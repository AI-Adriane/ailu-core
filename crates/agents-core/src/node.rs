//! Graph-runtime integration: run a [`ReActAgent`] as a node handler — the Rust
//! port of `@ailu/graph-sdk`'s `agent-node.ts` pattern.
//!
//! When the agent needs human approval and `suspend_for_approval` is set, the
//! handler returns [`NodeOutput::interrupt`] carrying the pending result into the
//! output channel — the run suspends cleanly. The control plane then patches the
//! [`APPROVED_TOOLS_CHANNEL`] (`GraphRuntime::update_state`) and resumes: the node
//! re-runs, the agent sees the granted tools, and execution proceeds.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ailu_graph_core::GraphState;
use ailu_graph_runtime::{NodeHandler, NodeOutput};
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

/// Build a [`NodeHandler`] that runs `agent` **once per item** in the `over_channel` array,
/// **concurrently**, and writes the per-item results — in INPUT order — into `join_at` as a JSON
/// array (ADR 0027 phase 4b, the `mapAgents` dynamic fan-out).
///
/// - Each spawn gets `item[i]` as its `input` and shares the run's channels as `State`, so a
///   sub-agent sees one item plus the common context.
/// - Spawns run concurrently (`join_all`), but the merge is by **input index** — `join_all`
///   preserves input order regardless of which spawn settles first — so the result is
///   deterministic and the run stays resumable.
/// - If any spawn flags `requires_human_review` and `suspend_for_approval` is set, the whole map
///   node suspends; on resume the node re-runs (the granted tools then execute). Granular
///   per-spawn resume is a follow-up.
/// - A per-spawn gateway error is surfaced as `{ "error": "<msg>" }` at that index, never failing
///   the whole node (parity with [`agent_node_handler`]).
pub fn map_node_handler(
    agent: Arc<ReActAgent>,
    node_id: String,
    over_channel: String,
    join_at: String,
    suspend_for_approval: bool,
) -> NodeHandler {
    Box::new(move |state: GraphState| {
        let agent = Arc::clone(&agent);
        let node_id = node_id.clone();
        let over_channel = over_channel.clone();
        let join_at = join_at.clone();
        Box::pin(async move {
            let approved = approved_tool_names(&state.channels);
            let items: Vec<Value> = match state.channels.get(&over_channel) {
                Some(Value::Array(items)) => items.clone(),
                // Absent / non-array → no spawns; write an empty array (deterministic no-op).
                _ => Vec::new(),
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
            let futures = items.iter().enumerate().map(|(index, item)| {
                agent.run_scoped(
                    item,
                    &state.channels,
                    &approved,
                    Some(index as u32),
                    Some(spawn_run_ids[index].as_str()),
                )
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

    /// An absent / empty `over_channel` → an empty array, no spawns (deterministic no-op).
    #[tokio::test]
    async fn map_node_with_no_items_writes_an_empty_array() {
        let gateway = DefaultLlmGateway::new(); // never called
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
            ),
        );
        let runtime = GraphRuntime::new(map_graph(), nodes, InMemoryConditionRegistry::new());
        let done = runtime
            .start(RunId::from("run-map-empty"), BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(done.status, GraphStatus::Completed);
        assert_eq!(
            done.channels.get("report").and_then(Value::as_array),
            Some(&vec![])
        );
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
