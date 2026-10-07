//! Graph-runtime integration: run a [`ReActAgent`] as a node handler — the Rust
//! port of `@ailu-ai/graph-sdk`'s `agent-node.ts` pattern.
//!
//! When the agent needs human approval and `suspend_for_approval` is set, the
//! handler returns [`NodeOutput::interrupt`] carrying the pending result into the
//! output channel — the run suspends cleanly. The control plane then patches the
//! [`APPROVED_TOOLS_CHANNEL`] (`GraphRuntime::update_state`) and resumes: the node
//! re-runs, the agent sees the granted tools, and execution proceeds.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ailu_graph_core::{FailureCategory, GraphState, NodeId};
use ailu_graph_runtime::{fan_out_items, NodeHandler, NodeOutput, RunEvent};
use serde_json::Value;

use crate::memory_tools::MEMORY_WRITES_CHANNEL;
use crate::react::ReActAgent;

/// Channel holding the names of tools whose human approval has been granted. The
/// control plane writes it before resuming a run that suspended for approval.
pub use ailu_graph_runtime::APPROVED_TOOLS_CHANNEL;

/// Reason carried by the interrupt an agent node raises when it needs approval.
pub const AGENT_APPROVAL_INTERRUPT: &str = "agent-approval-required";

/// Default channel an agent node writes its [`crate::react::AgentResult`] into.
pub const DEFAULT_AGENT_OUTPUT_CHANNEL: &str = "agentResult";

/// Build a [`NodeHandler`] that runs `agent` over the current state and writes its
/// result to `output_channel`.
///
/// - Approved tools are read from [`APPROVED_TOOLS_CHANNEL`] (a JSON array of
///   names; absence or `null` means none).
/// - With `suspend_for_approval`, a result flagged `requires_human_review`
///   suspends the run via [`NodeOutput::interrupt`], persisting the pending
///   result (including its `approvalRequests`) into the output channel.
/// - A gateway error is written to the output channel as `{ "error": "<msg>" }`
///   instead of failing the node: the runtime has no node-failure status or
///   retries yet, and surfacing the error as channel data keeps the run
///   deterministic and lets the graph route on it (e.g. into an alert path).
/// - When `todos_channel` is set and the agent called `writeTodos`, the
///   authoritative todo list is written into that channel in the **same** patch as
///   the result (one `NodeOutput::update` → one checkpoint), so the
///   after-every-node-completion invariant is preserved (ADR 0022/0023).
pub fn agent_node_handler(
    agent: Arc<ReActAgent>,
    output_channel: String,
    suspend_for_approval: bool,
    todos_channel: Option<String>,
) -> NodeHandler {
    Box::new(move |state: GraphState| {
        let agent = Arc::clone(&agent);
        let output_channel = output_channel.clone();
        let todos_channel = todos_channel.clone();
        Box::pin(async move {
            let approved = approved_tool_names(&state.channels);
            let run_id = logical_run_id(state.run_id.as_str());
            match agent
                .run(
                    &Value::Null,
                    &state.channels,
                    &approved,
                    Some(run_id.as_str()),
                )
                .await
            {
                Ok(result) => {
                    let requires_review = result.requires_human_review;
                    // Capture the todo list before `result` is consumed by `to_value`.
                    let todos_value = result
                        .todos
                        .as_ref()
                        .map(|todos| serde_json::to_value(todos).unwrap_or(Value::Null));
                    // ADR 0045 Stage 1b: the agent's durable memory write intents → the reserved
                    // `__memoryWrites` channel (fixed name, like `__memoryRecall`) for the control
                    // plane to drain into its durable store. Captured before `result` is consumed.
                    let memory_writes_value = result
                        .memory_writes
                        .as_ref()
                        .filter(|writes| !writes.is_empty())
                        .map(|writes| serde_json::to_value(writes).unwrap_or(Value::Null));
                    let value = serde_json::to_value(&result).unwrap_or(Value::Null);
                    let mut patch = BTreeMap::new();
                    patch.insert(output_channel, value);
                    // Same patch as the result → one checkpoint. Todos survive a
                    // suspension too (they are persisted even on the interrupt path).
                    if let (Some(channel), Some(todos)) = (todos_channel, todos_value) {
                        patch.insert(channel, todos);
                    }
                    if let Some(writes) = memory_writes_value {
                        patch.insert(MEMORY_WRITES_CHANNEL.to_owned(), writes);
                    }
                    if suspend_for_approval && requires_review {
                        NodeOutput::interrupt(AGENT_APPROVAL_INTERRUPT, patch)
                    } else {
                        NodeOutput::update(patch)
                    }
                }
                Err(error) => {
                    let mut patch = BTreeMap::new();
                    patch.insert(
                        output_channel,
                        serde_json::json!({ "error": error.to_string() }),
                    );
                    NodeOutput::update(patch)
                }
            }
        })
    })
}

/// Where a `mapAgents` node sends its per-spawn lifecycle events (ADR 0050): the four
/// `RunEvent::Spawn*` variants. Observational, like the token-delta [`crate::EventSink`]: the
/// bridge forwards them straight to the host's `on_event`, never onto the runtime `EventBus`.
pub type SpawnEventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// Build a [`NodeHandler`] that runs `agent` **once per item** in the `over_channel` array,
/// **concurrently**, and writes the per-item results — in INPUT order — into `join_at` as a JSON
/// array (ADR 0027 phase 4b, the `mapAgents` dynamic fan-out).
///
/// - Each spawn gets `item[i]` as its `input` and shares the run's channels as `State`, so a
///   sub-agent sees one item plus the common context.
/// - An absent, `null` or empty `over_channel` is no spawns (an empty array, a deterministic
///   no-op); a present value that is not an array fails the node (ADR 0050).
/// - Spawns run concurrently (`join_all`), but the merge is by **input index** — `join_all`
///   preserves input order regardless of which spawn settles first — so the result is
///   deterministic and the run stays resumable.
/// - If any spawn flags `requires_human_review` and `suspend_for_approval` is set, the whole map
///   node suspends; on resume the node re-runs (the granted tools then execute). Granular
///   per-spawn resume is a follow-up.
/// - A per-spawn gateway error is surfaced as `{ "error": "<msg>" }` at that index, never failing
///   the whole node (parity with [`agent_node_handler`]).
/// - With `spawn_events`, each spawn reports `SpawnStarted` then one of `SpawnCompleted`,
///   `SpawnFailed` or `SpawnSuspended` (ADR 0050). They change nothing in the node's output.
pub fn map_node_handler(
    agent: Arc<ReActAgent>,
    node_id: String,
    over_channel: String,
    join_at: String,
    suspend_for_approval: bool,
    spawn_events: Option<SpawnEventSink>,
) -> NodeHandler {
    Box::new(move |state: GraphState| {
        let agent = Arc::clone(&agent);
        let node_id = node_id.clone();
        let over_channel = over_channel.clone();
        let join_at = join_at.clone();
        let spawn_events = spawn_events.clone();
        Box::pin(async move {
            let approved = approved_tool_names(&state.channels);
            let items = match fan_out_items("mapAgents", &node_id, &state.channels, &over_channel) {
                Ok(items) => items,
                Err(error) => {
                    return NodeOutput::failure_with_category(error, FailureCategory::Permanent)
                }
            };
            // One sub-agent per item, run concurrently. `join_all` keeps INPUT order.
            // The `enumerate` index is the spawn id (ADR 0033 phase 13b): it equals the
            // deterministic merge order at `join_at`, so any token deltas a spawn streams
            // are demultiplexable by `spawnId` even though they arrive interleaved.
            // Each spawn's run_id (ADR 0043) is `{run_id}:{node_id}:{index}` — same
            // deterministic convention `subgraph_run_id`/the graph-runtime map fan-out use
            // (`runtime.rs`) — so concurrent spawns making a byte-identical LLM call still
            // journal (and later replay-match) distinctly. Built upfront (not per-future) so
            // each borrowed `&str` outlives the awaited futures below.
            let run_id = logical_run_id(state.run_id.as_str());
            let spawn_run_ids: Vec<String> = (0..items.len())
                .map(|index| format!("{run_id}:{node_id}:{index}"))
                .collect();
            let emit = |event: RunEvent| {
                if let Some(sink) = &spawn_events {
                    sink(event);
                }
            };
            let futures = items.iter().enumerate().map(|(index, item)| {
                let spawn = index as u32;
                let (run_id, node_id) = (state.run_id.clone(), NodeId::from(node_id.as_str()));
                let spawn_run_id = spawn_run_ids[index].as_str();
                let (agent, state, approved, emit) = (&agent, &state, &approved, &emit);
                async move {
                    emit(RunEvent::SpawnStarted {
                        run_id: run_id.clone(),
                        node_id: node_id.clone(),
                        spawn_id: spawn,
                        item_index: spawn,
                        item: item.clone(),
                        timestamp: wall_clock(),
                    });
                    let result = agent
                        .run_scoped(
                            item,
                            &state.channels,
                            approved,
                            Some(spawn),
                            Some(spawn_run_id),
                        )
                        .await;
                    emit(match &result {
                        Ok(res) if suspend_for_approval && res.requires_human_review => {
                            RunEvent::SpawnSuspended {
                                run_id,
                                node_id,
                                spawn_id: spawn,
                                item_index: spawn,
                                reason: AGENT_APPROVAL_INTERRUPT.to_owned(),
                                timestamp: wall_clock(),
                            }
                        }
                        Ok(res) => RunEvent::SpawnCompleted {
                            run_id,
                            node_id,
                            spawn_id: spawn,
                            item_index: spawn,
                            output: serde_json::to_value(res).unwrap_or(Value::Null),
                            usage: res
                                .usage
                                .as_ref()
                                .and_then(|usage| serde_json::to_value(usage).ok()),
                            timestamp: wall_clock(),
                        },
                        Err(error) => RunEvent::SpawnFailed {
                            run_id,
                            node_id,
                            spawn_id: spawn,
                            item_index: spawn,
                            error: error.to_string(),
                            timestamp: wall_clock(),
                        },
                    });
                    result
                }
            });
            let results = futures_util::future::join_all(futures).await;

            let mut outputs = Vec::with_capacity(results.len());
            let mut needs_review = false;
            for result in results {
                match result {
                    Ok(res) => {
                        needs_review |= res.requires_human_review;
                        outputs.push(serde_json::to_value(&res).unwrap_or(Value::Null));
                    }
                    Err(error) => {
                        outputs.push(serde_json::json!({ "error": error.to_string() }));
                    }
                }
            }

            let mut patch = BTreeMap::new();
            patch.insert(join_at, Value::Array(outputs));
            if suspend_for_approval && needs_review {
                NodeOutput::interrupt(AGENT_APPROVAL_INTERRUPT, patch)
            } else {
                NodeOutput::update(patch)
            }
        })
    })
}

/// Wall-clock millis since the epoch, for the observational spawn events only: they never
/// enter a checkpoint or a journal, so they must not read the runtime's (recorded) clock.
fn wall_clock() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

/// Strip every `fork:<n>` replay-fork segment (ADR 0043), wherever it falls in the id, not
/// just at the end. `GraphRuntime::replay_from` gives a replayed TOP-level run a NEW `run_id`
/// (`create_fork_run_id`, `runtime.rs`: `<run>:fork:<n>`), and a subgraph child's id is derived
/// by APPENDING `:{node_id}` onto whatever run_id it's given (`subgraph_run_id`) — so a child of
/// a replayed run reads `<run>:fork:<n>:<node_id>`, with the fork segment in the MIDDLE, not
/// trailing. For LLM request journal-tagging purposes a replay's calls are logically the SAME
/// run/subgraph as the record pass that produced the journal; untagged, `ReplayGateway`'s
/// request-equality match (which now includes `run_id`) would miss. This keeps tagging
/// fork-invariant while leaving `state.run_id` itself (checkpoints, subgraph child ids, event
/// routing) untouched everywhere else.
fn logical_run_id(run_id: &str) -> String {
    let segments: Vec<&str> = run_id.split(':').collect();
    let mut kept: Vec<&str> = Vec::with_capacity(segments.len());
    let mut i = 0;
    while i < segments.len() {
        let is_fork_pair = segments[i] == "fork"
            && segments
                .get(i + 1)
                .is_some_and(|seq| !seq.is_empty() && seq.bytes().all(|b| b.is_ascii_digit()));
        if is_fork_pair {
            i += 2;
        } else {
            kept.push(segments[i]);
            i += 1;
        }
    }
    kept.join(":")
}

/// Read the granted tool names from the channels — tolerant of an absent, `null`,
/// or non-string-array channel (all mean "nothing granted").
fn approved_tool_names(channels: &BTreeMap<String, Value>) -> HashSet<String> {
    match channels.get(APPROVED_TOOLS_CHANNEL) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => HashSet::new(),
    }
}

#[test]
fn logical_run_id_strips_one_or_several_fork_suffixes() {
    assert_eq!(logical_run_id("run-1"), "run-1");
    assert_eq!(logical_run_id("run-1:fork:7"), "run-1");
    assert_eq!(logical_run_id("run-1:fork:7:fork:2"), "run-1");
    // The fork marker can fall in the MIDDLE of a subgraph child's id — subgraph_run_id
    // appends `:{node_id}` onto whatever run_id it's given, fork suffix or not.
    assert_eq!(logical_run_id("run-1:fork:7:sub"), "run-1:sub");
    assert_eq!(logical_run_id("run-1:fork:7:sub:0"), "run-1:sub:0");
    // A node id that happens to contain "fork" but not the exact ":fork:<digits>" shape
    // is left alone — this is a suffix strip, not a substring scrub.
    assert_eq!(logical_run_id("run-1:forklift"), "run-1:forklift");
    assert_eq!(logical_run_id("run-1:node-a"), "run-1:node-a");
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ailu_graph_core::{
        ChannelDefinition, ChannelReducer, GraphDefinition, GraphId, GraphStatus, NodeDefinition,
        NodeId, NodeType, RunId,
    };
    use ailu_graph_runtime::{
        GraphRuntime, InMemoryConditionRegistry, InMemoryNodeRegistry, NodeRegistry,
    };
    use ailu_llm_gateway::{
        DefaultLlmGateway, LlmProvider, LlmResponse, LlmToolCall, LlmUsage, MockAdapter,
    };
    use serde_json::json;

    use super::*;
    use crate::tools::{sync_tool, InMemoryToolRegistry, ToolDefinition};

    fn replace_channel() -> ChannelDefinition {
        ChannelDefinition {
            channel_type: "json".to_owned(),
            reducer: ChannelReducer::Replace,
            default: None,
            no_log: false,
        }
    }

    fn agent_graph() -> GraphDefinition {
        GraphDefinition {
            id: GraphId::from("g-agent"),
            version: "0.0.0".to_owned(),
            name: "agent graph".to_owned(),
            recursion_limit: None,
            channels: [
                (DEFAULT_AGENT_OUTPUT_CHANNEL.to_owned(), replace_channel()),
                (APPROVED_TOOLS_CHANNEL.to_owned(), replace_channel()),
            ]
            .into_iter()
            .collect(),
            nodes: vec![NodeDefinition {
                id: NodeId::from("assistant"),
                node_type: NodeType::Agent,
                label: "assistant".to_owned(),
                subgraph_id: None,
                input_mapping: None,
                output_mapping: None,
                fan_out: None,
                map_subgraph: None,
                retry_policy: None,
                metadata: None,
            }],
            edges: vec![],
            entry_node_id: NodeId::from("assistant"),
            metadata: None,
        }
    }

    fn tool_use(name: &str) -> LlmResponse {
        LlmResponse {
            web_search: None,
            content: String::new(),
            tool_calls: Some(vec![LlmToolCall {
                id: "tu1".to_owned(),
                name: name.to_owned(),
                input: json!({}),
            }]),
            stop_reason: Some("tool_use".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    fn text(content: &str) -> LlmResponse {
        LlmResponse {
            web_search: None,
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("end_turn".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    /// Parity proof with the TS SDK test "suspends the run for approval, then
    /// executes the tool once granted on resume". The second scripted `tool_use`
    /// matters: after approval the node re-runs, so the agent must ask again to
    /// actually execute the now-granted tool.
    #[tokio::test]
    async fn suspends_for_approval_then_executes_after_grant() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);

        let mut tools = InMemoryToolRegistry::new();
        tools.register(
            ToolDefinition {
                name: "refund".to_owned(),
                description: "Issues a refund.".to_owned(),
                requires_approval: true,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
                approval_conditions: Vec::new(),
            },
            sync_tool(move |_input| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }),
        );

        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![
                tool_use("refund"),
                tool_use("refund"),
                text("FINAL: refunded"),
            ],
        )));

        let agent = ReActAgent::new("assistant", "refund agent", Arc::new(gateway))
            .with_tools(Arc::new(tools));

        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("assistant"),
            agent_node_handler(
                Arc::new(agent),
                DEFAULT_AGENT_OUTPUT_CHANNEL.to_owned(),
                true,
                None,
            ),
        );

        let runtime = GraphRuntime::new(agent_graph(), nodes, InMemoryConditionRegistry::new());
        let run_id = RunId::from("run-approval");

        let suspended = runtime
            .start(run_id.clone(), BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(suspended.status, GraphStatus::Suspended);
        let pending = suspended
            .channels
            .get(DEFAULT_AGENT_OUTPUT_CHANNEL)
            .expect("pending result persisted");
        assert_eq!(pending.get("requiresHumanReview"), Some(&json!(true)));
        assert_eq!(calls.load(Ordering::SeqCst), 0); // gated before execution

        runtime
            .update_state(
                &run_id,
                [(APPROVED_TOOLS_CHANNEL.to_owned(), json!(["refund"]))]
                    .into_iter()
                    .collect(),
            )
            .unwrap();

        let done = runtime.resume(&run_id).await.unwrap();
        assert_eq!(done.status, GraphStatus::Completed);
        assert_eq!(calls.load(Ordering::SeqCst), 1); // ran once approval was granted
    }

    #[tokio::test]
    async fn surfaces_a_gateway_error_into_the_output_channel() {
        // No adapter registered: the agent's first complete() fails.
        let gateway = DefaultLlmGateway::new();
        let agent = ReActAgent::new("assistant", "agent", Arc::new(gateway));

        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("assistant"),
            agent_node_handler(
                Arc::new(agent),
                DEFAULT_AGENT_OUTPUT_CHANNEL.to_owned(),
                false,
                None,
            ),
        );

        let runtime = GraphRuntime::new(agent_graph(), nodes, InMemoryConditionRegistry::new());
        let done = runtime
            .start(RunId::from("run-err"), BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Completed);
        let result = done
            .channels
            .get(DEFAULT_AGENT_OUTPUT_CHANNEL)
            .expect("error surfaced");
        let message = result
            .get("error")
            .and_then(Value::as_str)
            .expect("error message");
        assert!(!message.is_empty());
    }

    fn map_graph() -> GraphDefinition {
        GraphDefinition {
            id: GraphId::from("g-map"),
            version: "0.0.0".to_owned(),
            name: "map graph".to_owned(),
            recursion_limit: None,
            channels: [
                ("items".to_owned(), replace_channel()),
                ("report".to_owned(), replace_channel()),
                (APPROVED_TOOLS_CHANNEL.to_owned(), replace_channel()),
            ]
            .into_iter()
            .collect(),
            nodes: vec![NodeDefinition {
                id: NodeId::from("fanner"),
                node_type: NodeType::Agent,
                label: "fanner".to_owned(),
                subgraph_id: None,
                input_mapping: None,
                output_mapping: None,
                fan_out: None,
                map_subgraph: None,
                retry_policy: None,
                metadata: None,
            }],
            edges: vec![],
            entry_node_id: NodeId::from("fanner"),
            metadata: None,
        }
    }

    /// ADR 0027 phase 4b: `map_node_handler` runs one sub-agent per item and merges the
    /// per-item results into `join_at` as an array, in input order (deterministic).
    #[tokio::test]
    async fn map_node_runs_a_subagent_per_item_and_merges_into_an_array() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![text("FINAL: a"), text("FINAL: b")],
        )));
        let agent = ReActAgent::new("worker", "sub-agent", Arc::new(gateway));

        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("fanner"),
            map_node_handler(
                Arc::new(agent),
                "fanner".to_owned(),
                "items".to_owned(),
                "report".to_owned(),
                false,
                None,
            ),
        );

        let runtime = GraphRuntime::new(map_graph(), nodes, InMemoryConditionRegistry::new());
        let done = runtime
            .start(
                RunId::from("run-map"),
                [("items".to_owned(), json!(["x", "y"]))]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();

        assert_eq!(done.status, GraphStatus::Completed);
        let report = done
            .channels
            .get("report")
            .and_then(Value::as_array)
            .expect("report array written");
        assert_eq!(report.len(), 2);
        // Each element is a valid AgentResult (has the reasoning field).
        assert!(report[0].get("reasoning").is_some());
        assert!(report[1].get("reasoning").is_some());
    }

    /// A [`SpawnEventSink`] that keeps every event it gets.
    fn recording_sink() -> (SpawnEventSink, Arc<std::sync::Mutex<Vec<RunEvent>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let sink: SpawnEventSink = Arc::new(move |event| kept.lock().unwrap().push(event));
        (sink, seen)
    }

    fn map_runtime(
        agent: ReActAgent,
        suspend_for_approval: bool,
        sink: Option<SpawnEventSink>,
    ) -> GraphRuntime {
        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("fanner"),
            map_node_handler(
                Arc::new(agent),
                "fanner".to_owned(),
                "items".to_owned(),
                "report".to_owned(),
                suspend_for_approval,
                sink,
            ),
        );
        GraphRuntime::new(map_graph(), nodes, InMemoryConditionRegistry::new())
    }

    /// `(type, spawnId, itemIndex)` of each event, sorted (spawns run concurrently).
    fn spawn_summary(events: &[RunEvent]) -> Vec<(&'static str, u32, u32)> {
        let mut summary: Vec<_> = events
            .iter()
            .map(|event| match event {
                RunEvent::SpawnStarted {
                    spawn_id,
                    item_index,
                    ..
                } => ("started", *spawn_id, *item_index),
                RunEvent::SpawnCompleted {
                    spawn_id,
                    item_index,
                    ..
                } => ("completed", *spawn_id, *item_index),
                RunEvent::SpawnFailed {
                    spawn_id,
                    item_index,
                    ..
                } => ("failed", *spawn_id, *item_index),
                RunEvent::SpawnSuspended {
                    spawn_id,
                    item_index,
                    ..
                } => ("suspended", *spawn_id, *item_index),
                other => panic!("not a spawn event: {other:?}"),
            })
            .collect();
        summary.sort();
        summary
    }

    /// ADR 0050: N items → N `spawn_started` (with the item) + N `spawn_completed` (with the
    /// output and usage), each spawn's start before its end; the node's own events unchanged.
    #[tokio::test]
    async fn map_node_reports_each_spawn_started_then_completed() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![text("FINAL: a"), text("FINAL: b")],
        )));
        let (sink, seen) = recording_sink();
        let runtime = map_runtime(
            ReActAgent::new("worker", "sub-agent", Arc::new(gateway)),
            false,
            Some(sink),
        );
        let done = runtime
            .start(
                RunId::from("run-spawns"),
                [("items".to_owned(), json!(["x", "y"]))]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Completed);

        let events = seen.lock().unwrap().clone();
        assert_eq!(
            spawn_summary(&events),
            vec![
                ("completed", 0, 0),
                ("completed", 1, 1),
                ("started", 0, 0),
                ("started", 1, 1)
            ]
        );
        for spawn in 0..2u32 {
            let position = |completed: bool| {
                events.iter().position(|event| match event {
                    RunEvent::SpawnStarted { spawn_id, .. } => !completed && *spawn_id == spawn,
                    RunEvent::SpawnCompleted { spawn_id, .. } => completed && *spawn_id == spawn,
                    _ => false,
                })
            };
            assert!(position(false) < position(true));
        }
        for event in &events {
            match event {
                RunEvent::SpawnStarted {
                    run_id,
                    node_id,
                    spawn_id,
                    item,
                    ..
                } => {
                    assert_eq!(
                        (run_id.as_str(), node_id.as_str()),
                        ("run-spawns", "fanner")
                    );
                    assert_eq!(item, &json!(["x", "y"][*spawn_id as usize]));
                }
                RunEvent::SpawnCompleted {
                    spawn_id,
                    output,
                    usage,
                    ..
                } => {
                    let report = done.channels["report"].as_array().unwrap();
                    assert_eq!(output, &report[*spawn_id as usize]);
                    assert!(usage
                        .as_ref()
                        .is_some_and(|u| u.get("promptTokens").is_some()));
                }
                _ => {}
            }
        }
        // The node's own lifecycle is untouched: the bus never sees a spawn event.
        let bus = runtime.events().events();
        assert!(bus.iter().all(|event| !matches!(
            event,
            RunEvent::SpawnStarted { .. }
                | RunEvent::SpawnCompleted { .. }
                | RunEvent::SpawnFailed { .. }
                | RunEvent::SpawnSuspended { .. }
        )));
        assert!(bus
            .iter()
            .any(|e| matches!(e, RunEvent::NodeStarted { .. })));
        assert!(bus
            .iter()
            .any(|e| matches!(e, RunEvent::NodeCompleted { .. })));
    }

    /// ADR 0050: a spawn whose agent errors reports `spawn_failed`; the node still completes.
    #[tokio::test]
    async fn map_node_reports_a_failing_spawn() {
        let (sink, seen) = recording_sink();
        // No adapter registered: every spawn's first complete() fails.
        let runtime = map_runtime(
            ReActAgent::new("worker", "sub-agent", Arc::new(DefaultLlmGateway::new())),
            false,
            Some(sink),
        );
        let done = runtime
            .start(
                RunId::from("run-spawn-fail"),
                [("items".to_owned(), json!(["x"]))].into_iter().collect(),
            )
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Completed);
        let events = seen.lock().unwrap().clone();
        assert_eq!(
            spawn_summary(&events),
            vec![("failed", 0, 0), ("started", 0, 0)]
        );
        assert!(matches!(&events[1], RunEvent::SpawnFailed { error, .. } if !error.is_empty()));
    }

    /// ADR 0050: a spawn that needs approval under `suspendForApproval` reports
    /// `spawn_suspended` with the run's interrupt reason.
    #[tokio::test]
    async fn map_node_reports_a_spawn_suspended_for_approval() {
        let mut tools = InMemoryToolRegistry::new();
        tools.register(
            ToolDefinition {
                name: "refund".to_owned(),
                description: "Issues a refund.".to_owned(),
                requires_approval: true,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
                approval_conditions: Vec::new(),
            },
            sync_tool(|_input| Ok(json!({ "ok": true }))),
        );
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![tool_use("refund")],
        )));
        let agent =
            ReActAgent::new("worker", "sub-agent", Arc::new(gateway)).with_tools(Arc::new(tools));
        let (sink, seen) = recording_sink();
        let runtime = map_runtime(agent, true, Some(sink));
        let suspended = runtime
            .start(
                RunId::from("run-spawn-gate"),
                [("items".to_owned(), json!(["x"]))].into_iter().collect(),
            )
            .await
            .unwrap();
        assert_eq!(suspended.status, GraphStatus::Suspended);
        let events = seen.lock().unwrap().clone();
        assert_eq!(
            spawn_summary(&events),
            vec![("started", 0, 0), ("suspended", 0, 0)]
        );
        assert!(matches!(
            &events[1],
            RunEvent::SpawnSuspended { reason, .. } if reason == AGENT_APPROVAL_INTERRUPT
        ));
    }

    /// ADR 0050 (ailu#2033): a present `over_channel` that is not an array fails the node with
    /// a message naming the node, the channel and the type; nothing is spawned.
    #[tokio::test]
    async fn map_node_over_a_non_array_fails_the_node_clearly() {
        let (sink, seen) = recording_sink();
        let runtime = map_runtime(
            ReActAgent::new("worker", "sub-agent", Arc::new(DefaultLlmGateway::new())),
            false,
            Some(sink),
        );
        let done = runtime
            .start(
                RunId::from("run-map-str"),
                [("items".to_owned(), json!("[\"a\",\"b\"]"))]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Failed);
        let expected =
            "mapAgents node 'fanner': overChannel 'items' must be a JSON array, got string";
        assert!(runtime.events().events().iter().any(|e| matches!(
            e,
            RunEvent::NodeFailed { error, .. } if error == expected
        )));
        assert!(seen.lock().unwrap().is_empty());
    }

    /// ADR 0050: an absent, `null` or empty `over_channel` spawns nothing and does not fail.
    #[tokio::test]
    async fn map_node_with_absent_null_or_empty_items_spawns_nothing() {
        for initial in [None, Some(Value::Null), Some(json!([]))] {
            let (sink, seen) = recording_sink();
            let runtime = map_runtime(
                ReActAgent::new("worker", "sub-agent", Arc::new(DefaultLlmGateway::new())),
                false,
                Some(sink),
            );
            let done = runtime
                .start(
                    RunId::from("run-map-none"),
                    initial
                        .into_iter()
                        .map(|v| ("items".to_owned(), v))
                        .collect(),
                )
                .await
                .unwrap();
            assert_eq!(done.status, GraphStatus::Completed);
            assert_eq!(done.channels.get("report"), Some(&json!([])));
            assert!(seen.lock().unwrap().is_empty());
        }
    }

    /// A gateway that records the `run_id` of every request it sees (ADR 0043).
    #[derive(Default)]
    struct RunIdCapturingGateway {
        seen: std::sync::Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ailu_llm_gateway::LlmGateway for RunIdCapturingGateway {
        async fn complete(
            &self,
            request: ailu_llm_gateway::LlmRequest,
        ) -> Result<LlmResponse, ailu_llm_gateway::LlmError> {
            self.seen.lock().expect("lock").push(request.run_id);
            Ok(LlmResponse {
                web_search: None,
                content: "FINAL: done".to_owned(),
                tool_calls: None,
                stop_reason: Some("end_turn".to_owned()),
                usage: LlmUsage::default(),
                model: "mock".to_owned(),
                provider: LlmProvider::Anthropic,
                content_blocks: None,
            })
        }
    }

    /// Each `mapAgents` spawn's `LlmRequest.run_id` (ADR 0043) is the deterministic
    /// `{run_id}:{node_id}:{index}` — the SAME convention `subgraph_run_id`/the
    /// graph-runtime map fan-out use (`runtime.rs`) — so concurrent spawns making a
    /// byte-identical LLM call still journal distinctly.
    #[tokio::test]
    async fn map_node_tags_each_spawns_request_with_its_deterministic_run_id() {
        let gateway = Arc::new(RunIdCapturingGateway::default());
        let agent = ReActAgent::new("worker", "sub-agent", gateway.clone());

        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("fanner"),
            map_node_handler(
                Arc::new(agent),
                "fanner".to_owned(),
                "items".to_owned(),
                "report".to_owned(),
                false,
                None,
            ),
        );

        let runtime = GraphRuntime::new(map_graph(), nodes, InMemoryConditionRegistry::new());
        runtime
            .start(
                RunId::from("run-map-ids"),
                [("items".to_owned(), json!(["x", "y"]))]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();

        let mut seen: Vec<Option<String>> = gateway.seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                Some("run-map-ids:fanner:0".to_owned()),
                Some("run-map-ids:fanner:1".to_owned()),
            ]
        );
    }

    /// `writeTodos` persists into the durable todos channel in the SAME checkpointed
    /// update as the result — one node completion, one checkpoint (ADR 0022/0023).
    #[tokio::test]
    async fn write_todos_persists_into_the_durable_channel() {
        use crate::todos::{write_todos_tool, TODOS_CHANNEL};

        let mut registry = InMemoryToolRegistry::new();
        let (definition, handler) = write_todos_tool();
        registry.register(definition, handler);

        let write_todos_call = LlmResponse {
            web_search: None,
            content: String::new(),
            tool_calls: Some(vec![LlmToolCall {
                id: "tu1".to_owned(),
                name: "writeTodos".to_owned(),
                input: json!({
                    "todos": [
                        { "text": "scope", "status": "completed" },
                        { "text": "build", "status": "in_progress" }
                    ]
                }),
            }]),
            stop_reason: Some("tool_use".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        };

        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![write_todos_call, text("FINAL: planned")],
        )));

        let agent = ReActAgent::new("assistant", "planner", Arc::new(gateway))
            .with_tools(Arc::new(registry));

        // A graph that declares the durable todos channel alongside the output channel.
        let graph = GraphDefinition {
            id: GraphId::from("g-todos"),
            version: "0.0.0".to_owned(),
            name: "todos graph".to_owned(),
            recursion_limit: None,
            channels: [
                (DEFAULT_AGENT_OUTPUT_CHANNEL.to_owned(), replace_channel()),
                (APPROVED_TOOLS_CHANNEL.to_owned(), replace_channel()),
                (TODOS_CHANNEL.to_owned(), replace_channel()),
            ]
            .into_iter()
            .collect(),
            nodes: vec![NodeDefinition {
                id: NodeId::from("assistant"),
                node_type: NodeType::Agent,
                label: "assistant".to_owned(),
                subgraph_id: None,
                input_mapping: None,
                output_mapping: None,
                fan_out: None,
                map_subgraph: None,
                retry_policy: None,
                metadata: None,
            }],
            edges: vec![],
            entry_node_id: NodeId::from("assistant"),
            metadata: None,
        };

        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            NodeId::from("assistant"),
            agent_node_handler(
                Arc::new(agent),
                DEFAULT_AGENT_OUTPUT_CHANNEL.to_owned(),
                false,
                Some(TODOS_CHANNEL.to_owned()),
            ),
        );

        let runtime = GraphRuntime::new(graph, nodes, InMemoryConditionRegistry::new());
        let done = runtime
            .start(RunId::from("run-todos"), BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Completed);

        let todos = done
            .channels
            .get(TODOS_CHANNEL)
            .and_then(Value::as_array)
            .expect("todos persisted into the durable channel");
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].get("id").and_then(Value::as_str), Some("todo-1"));
        assert_eq!(
            todos[0].get("status").and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(
            todos[1].get("status").and_then(Value::as_str),
            Some("in_progress")
        );
    }
}
