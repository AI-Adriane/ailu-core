//! The wire spec the SDK sends across the napi boundary, plus the outcome it
//! gets back. All camelCase, matching the TS SDK and `@ailu-ai/graph-core`.

use std::collections::BTreeMap;

use ailu_agents_core::ApprovalRequestItem;
use ailu_fs_backend::FsPermVerb;
use ailu_graph_core::{GraphDefinition, GraphState};
use ailu_llm_gateway::ModelTier;
use ailu_skills::Skill;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration for one agent node, keyed in [`EngineSpec::agents`] by node id.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    /// `"openai" | "anthropic" | "mistral"` — drives the provider slot used in the
    /// [`ailu_llm_gateway::LlmRequest`]. The actual adapter is chosen from env
    /// (see `bridge::build_gateway`); this only sets the request's `provider`.
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Optional capability tier (`"frontier" | "balanced" | "fast" | "creative"`).
    /// When set and no explicit `model` is given, the concrete model is resolved by
    /// the [`ailu_llm_gateway::ModelPolicy`] against the providers available in the
    /// process environment (see `bridge::resolve_agent_model`). An explicit `model`
    /// always wins over `tier`.
    #[serde(default)]
    pub tier: Option<ModelTier>,
    /// A custom OpenAI-compatible endpoint (`model.openaiCompatible({ baseURL })` — vLLM, LM
    /// Studio, a gateway, …). When set, the agent is served by the OpenAI-wire adapter pointed at
    /// this URL instead of `api.openai.com`, and its key comes ONLY from [`Self::api_key_env`]:
    /// `OPENAI_API_KEY` and tenant `provider_keys` belong to OpenAI and are never sent here.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Name of the environment variable holding the key for [`Self::base_url`]. Unset means the
    /// endpoint is keyless; set but empty/missing fails the build.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    /// Names of tools this agent may call. A name in [`EngineSpec::js_tool_names`]
    /// is backed by a JS `execute` fn; otherwise it is a no-op stub tool.
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// What the LLM is told about each tool in [`Self::tool_names`]: its description and the
    /// JSON Schema of its input. A tool with no entry here is advertised by name only.
    #[serde(default)]
    pub tool_specs: Vec<ToolSpec>,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// When true, a tool that needs approval suspends the run (the agent-node
    /// dynamic-interrupt). When false, gated tools simply stop the agent loop.
    #[serde(default)]
    pub suspend_for_approval: bool,
    /// Tools (by name) that require human approval before the agent may run them.
    #[serde(default)]
    pub approval_tool_names: Vec<String>,
    /// The channel the agent writes its `AgentResult` into. Defaults to the
    /// agents-core `DEFAULT_AGENT_OUTPUT_CHANNEL` (`agentResult`).
    #[serde(default)]
    pub output_channel: Option<String>,
    /// Token-efficiency knobs (ADR 0014). `"terse"` appends a compact-output directive to
    /// the system prompt (cuts output tokens on prose; lossy — not for code). `None` = off.
    #[serde(default)]
    pub output_style: Option<String>,
    /// Cap (in chars) on the serialized `State` the agent injects into its first message —
    /// don't re-feed an unbounded channel map. `None` = no cap.
    #[serde(default)]
    pub context_budget: Option<u32>,
    /// Durable channel the agent's `writeTodos` list is persisted into (ADR 0022/0023,
    /// phase 1). When set and the agent has the `writeTodos` tool, the node handler
    /// writes the authoritative todo list here in the same checkpointed update as the
    /// result. `None` = no durable todos sink (the list still appears in the result).
    #[serde(default)]
    pub todos_channel: Option<String>,
    /// Opt this agent into the governed virtual filesystem tools (ADR 0024 phase 2b):
    /// `read_file`/`ls`/`glob`/`grep`/`write_file`/`edit_file`/`delete_file`/`move_file`,
    /// run-scoped and enforced by [`EngineSpec::fs_policy`] (fail-closed). Default off.
    #[serde(default)]
    pub enable_fs: bool,
    /// The SDK-resolved EFFICIENCY middleware list (ADR 0025 phase 3d): the SDK expands a
    /// `profile` + the user's explicit `middleware[]` + the legacy `output_style`/
    /// `context_budget` knobs into one ordered list of `{kind, params}` data entries, which
    /// the bridge turns into `push_efficiency` calls. The GOVERNED layer (redaction, the
    /// approval gate, fs policy) is **never** carried here — the bridge injects it — and a
    /// governance kind appearing in this list is ignored by the bridge's `_ => {}` arm (the
    /// runtime enforcer), never honoured. Default empty.
    #[serde(default)]
    pub resolved_middleware: Vec<MiddlewareSpec>,
    /// Channel carrying this run's multimodal input blocks (ADR 0030 phase 9e): a
    /// `Vec<ContentBlock>` value. When set, the agent's seed message becomes multimodal
    /// (text Input/State digest + the media blocks) and the channel is excluded from the
    /// stringified State. `None` = text-only seed (default).
    #[serde(default)]
    pub input_blocks_channel: Option<String>,
    /// The only channels this agent is shown in its seed `State` (context isolation). `None` =
    /// every channel.
    #[serde(default)]
    pub visible_channels: Option<Vec<String>>,
    /// Governed long-term memory (ADR 0026 phase 11). When set, a sealed `MemoryMiddleware`
    /// recalls from this namespace before the run (vector) and persists after, attributed. The
    /// namespace + principal are bridge-sealed (never user-routable). `None` = no memory.
    #[serde(default)]
    pub memory: Option<MemorySpec>,
    /// Governed skills — progressive disclosure (ADR 0035 phase 12). When set, a sealed
    /// `SkillMiddleware` selects skills (explicit `required` pins + advisory vector top-k) from
    /// this namespace before the run and prepends their bodies to the seed. The namespace is
    /// bridge-sealed; capability-granting (`requires`) skills are withheld until granted. Applies
    /// to this agent AND its `mapAgents`/`taskNode` sub-agents (same build path → deepagents
    /// parity). `None` = no skills.
    #[serde(default)]
    pub skills: Option<SkillSpec>,
}

/// How a tool is advertised to the LLM: the author's description and the JSON Schema of its
/// input (the SDK's `ToolDefinition.description` / `jsonSchema`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
}

/// The agentNode `skills` overlay (ADR 0035 phase 12). `required` is the explicit `name@version`
/// pin list (the must-apply playbooks); `advisory_k` caps the vector-selected advisory set. The
/// namespace is tenant-scoped (the control plane validates access).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSpec {
    pub namespace: String,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub advisory_k: Option<u32>,
}

/// The agentNode `memory` overlay (ADR 0026 phase 11). Quality knobs (`top_k`, `recall`) are
/// author-tunable; the namespace is tenant-scoped (the control plane validates access).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySpec {
    pub namespace: String,
    #[serde(default)]
    pub top_k: Option<u32>,
    /// `"vector" | "graph" | "both"` (default `both`).
    #[serde(default)]
    pub recall: Option<String>,
}

/// One entry of [`AgentSpec::resolved_middleware`]: an EFFICIENCY middleware the bridge
/// builds by `kind` (`"compress" | "terse" | "contextBudget"`), with kind-specific `params`
/// (e.g. `{ "chars": 4000 }` for `contextBudget`). Mirrors [`ComponentNodeSpec`]'s
/// data-driven shape. Unknown / governance kinds are ignored by the bridge (never applied).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiddlewareSpec {
    pub kind: String,
    #[serde(default)]
    pub params: Value,
}

/// One per-path permission rule (ADR 0024), compiled into the engine's
/// [`ailu_fs_backend::StaticPathPolicy`]. `verb` is `deny|read|write|gate`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsPolicyRule {
    pub glob: String,
    pub verb: FsPermVerb,
}

/// A `mapAgents` node (ADR 0027 phase 4b — dynamic fan-out), keyed in
/// [`EngineSpec::map_agents`] by node id. The bridge runs `agent` once per item in the
/// `over_channel` array (concurrently), and writes the per-item results — in input order — into
/// `join_at` as a JSON array. Each spawn gets `item[i]` as its `input` and shares the run's
/// channels. `suspend_for_approval` makes a gated spawn suspend the whole map.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapAgentSpec {
    pub over_channel: String,
    pub join_at: String,
    pub agent: AgentSpec,
    #[serde(default)]
    pub suspend_for_approval: bool,
}

/// A graph node backed by a native Rust component, keyed in
/// [`EngineSpec::component_nodes`] by node id. The bridge builds the runtime handler
/// from [`ailu_components::ComponentRegistry::build_handler`] using `kind` +
/// `params`, so the component runs natively on Rust (it is **not** routed to the JS
/// `on_node` seam). `kind` is one of `ComponentRegistry::kinds()` (e.g.
/// `"promptBuilder"`, `"router"`, `"retriever"`); `params` is the component's
/// configuration object, validated up front at build time.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentNodeSpec {
    pub kind: String,
    #[serde(default)]
    pub params: Value,
}

/// One tool a human has granted on the approve path, carrying the governance
/// provenance the engine guard-rail validates: the principal who *requested* it and
/// the principal who *resolved* it must differ (no self-approval). The control plane
/// is the source of truth — it only ever sends tools an [`ailu_approval_engine`]
/// decision already approved — but the bridge re-checks the invariant before writing
/// the tool into the resume channel (defence in depth).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedTool {
    /// The tool name (the approval subject's `tool:<name>`, stripped of its prefix).
    pub name: String,
    /// The principal that *filed* the approval request (the agent/node).
    #[serde(default)]
    pub requested_by: String,
    /// The principal that *resolved* (approved) it — must be present and differ from
    /// `requested_by`, or the bridge rejects the resume.
    #[serde(default)]
    pub resolved_by: String,
    /// The content-scoped grant key (ADR 0024 phase 2c): `"<name>#<sha256(input)>"` for
    /// a guarded fs write, pinning the grant to the exact call. When set, the bridge
    /// writes THIS key (not the bare name) into the `__approvedTools` channel, so only
    /// the approved path+content unlocks. `None` for an ordinary name-only grant.
    #[serde(default)]
    pub key: Option<String>,
}

/// The full spec for a run/resume/approve call.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSpec {
    pub graph: GraphDefinition,
    /// Child graphs that `subgraph`-type nodes resolve into, keyed by their own
    /// graph id. Their node handlers / agent / component configs are flattened into
    /// the same `jsNodeIds` / `agents` / `componentNodes` maps (by global node id),
    /// and their conditional edges into the same condition registry — so the bridge
    /// registers them alongside the parent's. Empty for a graph with no subgraphs.
    #[serde(default)]
    pub subgraphs: Vec<GraphDefinition>,
    /// Dynamic-message inbox to pre-queue before the run: per node id, a FIFO list of
    /// inputs (`send`). Each is consumed by that node's next execution via the reserved
    /// `__injected` channel. Empty for runs that don't use `send`.
    #[serde(default)]
    pub inbox: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub run_id: Option<String>,
    /// Opt-in per-token streaming (ADR 0033, phase 13). When `true`, agent nodes drive
    /// `gateway.stream()` and emit observational `RunEvent::TokenDelta` events over the
    /// `on_event` sink (bypassing the durable EventBus). When `false`/absent — the default —
    /// agents call `gateway.complete()` and the run is byte-identical to before. A consumer
    /// only sets this when it actually renders a live token view.
    #[serde(default)]
    pub stream_tokens: bool,
    #[serde(default)]
    pub initial_data: BTreeMap<String, Value>,
    /// Serialized [`GraphState`] of a suspended run — required by resume/approve,
    /// ignored by start.
    #[serde(default)]
    pub state: Option<GraphState>,
    /// Replay-as-evidence (ADR 0038): a recorded run journal — `{ decisions, clock }` as JSON
    /// — fed on `Entry::Replay` so the re-execution re-serves the original LLM outputs (via
    /// a `ReplayGateway`) and timestamps (via a `RecordedClock`) instead of re-sampling.
    /// Absent for a normal run.
    #[serde(default)]
    pub replay_journal: Option<String>,
    /// Tools a human has granted on the approve OR resume path: each carries its
    /// `{ name, requestedBy, resolvedBy }` provenance. The bridge validates the
    /// no-self-approval invariant per tool, then writes only the validated *names* into
    /// the `__approvedTools` channel before resuming — on BOTH `Entry::Approve` and the
    /// production catalog `Entry::Resume`, so a malformed/forged resume cannot unlock a
    /// self-approved tool. `#[serde(default)]` keeps a spec that omits it (start, or an
    /// ordinary resume past a non-approval gate) deserializing to an empty list.
    #[serde(default)]
    pub approved_tools: Vec<ApprovedTool>,
    /// Per-node agent configuration, keyed by node id.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentSpec>,
    /// Per-node `mapAgents` dynamic-fan-out configuration (ADR 0027 phase 4b), keyed by node id.
    #[serde(default)]
    pub map_agents: BTreeMap<String, MapAgentSpec>,
    /// Per-node native component configuration, keyed by node id. Such a node runs a
    /// Rust [`ailu_components`] handler (built at assemble time) instead of the JS
    /// seam, even if its id also appears in [`Self::js_node_ids`].
    #[serde(default)]
    pub component_nodes: BTreeMap<String, ComponentNodeSpec>,
    /// Node ids whose handler is a JS closure (action / tool / custom nodes).
    #[serde(default)]
    pub js_node_ids: Vec<String>,
    /// Tool names whose `execute` is a JS closure.
    #[serde(default)]
    pub js_tool_names: Vec<String>,
    /// Per-provider API keys the control plane injects for this run (ADR 0010), keyed by
    /// provider id (`"openai" | "anthropic" | "mistral" | "google" | "openrouter" | ...`).
    /// `bridge::build_gateway` uses a key here when present, falling back to the process env —
    /// so admin-managed, per-tenant keys reach the engine without going through `.env`. Empty
    /// for runs that rely on env (dev/self-host with deploy secrets).
    #[serde(default)]
    pub provider_keys: BTreeMap<String, String>,
    /// Per-path filesystem permission rules (ADR 0024 phase 2b), compiled into the
    /// run's `StaticPathPolicy` and applied to every fs-enabled agent. Empty = the
    /// fail-closed default (read-only everywhere; writes need an explicit rule).
    #[serde(default)]
    pub fs_policy: Vec<FsPolicyRule>,
    /// The tenant's governed skills for this run (ADR 0049 B-3). The control plane passes its skill
    /// store here; the bridge builds a run-scoped, tenant-isolated `InMemorySkillStore` from it that
    /// every agent's `SkillMiddleware` selects from. Empty = the OSS shared in-memory store (no skills).
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// What a run/resume/approve call returns to JS: the final state plus, when
/// suspended, the pending approvals and the state to feed back into approve.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    /// The final (or suspended) [`GraphState`].
    pub state: GraphState,
    /// `"running" | "suspended" | "completed" | "failed"` etc. — the state's
    /// status, surfaced at top level for convenience.
    pub status: String,
    /// Pending tool approvals gathered from the agent output channels when the run
    /// suspended for approval. Empty when not suspended on an approval gate.
    pub pending_approvals: Vec<ApprovalRequestItem>,
    /// Replay-as-evidence (ADR 0038): the recorded LLM I/O + clock journal for this run,
    /// `{ decisions, clock }` as JSON, when it executed in record mode (env `AILU_LLM_RECORD`).
    /// `None` otherwise. The control plane persists it to re-feed a later verify-replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_journal: Option<String>,
    /// Replay-as-evidence (ADR 0040): the run's **entry** [`GraphState`] (initial state, before the
    /// entry node ran), surfaced only on a record-mode `Entry::Start`. `None` otherwise. The control
    /// plane persists it as the checkpoint a later verify-replay seeds `replay_from` from, so the
    /// run can be re-derived from its very start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_state: Option<GraphState>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_graph_json() -> Value {
        json!({
            "id": "g",
            "version": "0.0.0",
            "name": "g",
            "channels": {},
            "nodes": [{ "id": "a", "type": "action", "label": "a" }],
            "edges": [],
            "entryNodeId": "a"
        })
    }

    #[test]
    fn deserializes_a_minimal_start_spec() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "runId": "run-1",
            "jsNodeIds": ["a"]
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        assert_eq!(spec.run_id.as_deref(), Some("run-1"));
        assert_eq!(spec.js_node_ids, vec!["a".to_owned()]);
        assert!(spec.agents.is_empty());
        assert!(spec.state.is_none());
        assert!(spec.initial_data.is_empty());
    }

    #[test]
    fn deserializes_approved_tools_with_provenance() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "approvedTools": [
                { "name": "refund", "requestedBy": "assistant", "resolvedBy": "alice" }
            ]
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        assert_eq!(spec.approved_tools.len(), 1);
        let tool = &spec.approved_tools[0];
        assert_eq!(tool.name, "refund");
        assert_eq!(tool.requested_by, "assistant");
        assert_eq!(tool.resolved_by, "alice");
    }

    #[test]
    fn approved_tools_default_to_empty_and_provenance_defaults_to_blank() {
        // A start/resume spec omits `approvedTools` entirely (default empty); a tool
        // object may omit `requestedBy`/`resolvedBy` (both default to "" — the bridge
        // then treats the blank resolver as a self-approval violation).
        let spec: EngineSpec =
            serde_json::from_value(json!({ "graph": minimal_graph_json() })).expect("spec parses");
        assert!(spec.approved_tools.is_empty());

        let partial: ApprovedTool =
            serde_json::from_value(json!({ "name": "refund" })).expect("tool parses");
        assert_eq!(partial.requested_by, "");
        assert_eq!(partial.resolved_by, "");
    }

    #[test]
    fn deserializes_an_agent_spec_with_camel_case_keys() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "agents": {
                "assistant": {
                    "provider": "anthropic",
                    "model": "claude-x",
                    "system": "be helpful",
                    "toolNames": ["refund"],
                    "maxIterations": 3,
                    "suspendForApproval": true,
                    "approvalToolNames": ["refund"]
                }
            },
            "jsToolNames": ["refund"]
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        let agent = spec.agents.get("assistant").expect("agent present");
        assert_eq!(agent.provider, "anthropic");
        assert_eq!(agent.model.as_deref(), Some("claude-x"));
        assert_eq!(agent.tool_names, vec!["refund".to_owned()]);
        assert_eq!(agent.max_iterations, Some(3));
        assert!(agent.suspend_for_approval);
        assert_eq!(agent.approval_tool_names, vec!["refund".to_owned()]);
        assert_eq!(spec.js_tool_names, vec!["refund".to_owned()]);
        assert!(agent.tier.is_none());
        assert!(spec.component_nodes.is_empty());
    }

    #[test]
    fn deserializes_the_todos_channel_from_camel_case() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "agents": {
                "assistant": {
                    "provider": "anthropic",
                    "toolNames": ["writeTodos"],
                    "todosChannel": "__todos"
                }
            }
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        let agent = spec.agents.get("assistant").expect("agent present");
        assert_eq!(agent.todos_channel.as_deref(), Some("__todos"));
        assert_eq!(agent.tool_names, vec!["writeTodos".to_owned()]);

        // Omitted → None (no durable sink).
        let bare: EngineSpec = serde_json::from_value(json!({
            "graph": minimal_graph_json(),
            "agents": { "assistant": { "provider": "anthropic" } }
        }))
        .expect("spec parses");
        assert!(bare
            .agents
            .get("assistant")
            .expect("agent present")
            .todos_channel
            .is_none());
    }

    #[test]
    fn deserializes_fs_enablement_and_policy_from_camel_case() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "agents": {
                "assistant": { "provider": "anthropic", "enableFs": true }
            },
            "fsPolicy": [
                { "glob": "scratch/**", "verb": "write" },
                { "glob": "secret/**", "verb": "deny" }
            ]
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        assert!(
            spec.agents
                .get("assistant")
                .expect("agent present")
                .enable_fs
        );
        assert_eq!(spec.fs_policy.len(), 2);
        assert_eq!(spec.fs_policy[0].glob, "scratch/**");
        assert_eq!(spec.fs_policy[0].verb, FsPermVerb::Write);
        assert_eq!(spec.fs_policy[1].verb, FsPermVerb::Deny);

        // Omitted → fs disabled + empty policy (fail-closed default applies).
        let bare: EngineSpec =
            serde_json::from_value(json!({ "graph": minimal_graph_json() })).expect("parses");
        assert!(bare.fs_policy.is_empty());
    }

    #[test]
    fn deserializes_resolved_middleware_from_camel_case() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "agents": {
                "assistant": {
                    "provider": "anthropic",
                    "resolvedMiddleware": [
                        { "kind": "compress" },
                        { "kind": "terse" },
                        { "kind": "contextBudget", "params": { "chars": 4000 } }
                    ]
                }
            }
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        let agent = spec.agents.get("assistant").expect("agent present");
        assert_eq!(agent.resolved_middleware.len(), 3);
        assert_eq!(agent.resolved_middleware[0].kind, "compress");
        assert_eq!(agent.resolved_middleware[2].kind, "contextBudget");
        assert_eq!(
            agent.resolved_middleware[2]
                .params
                .get("chars")
                .and_then(Value::as_u64),
            Some(4000)
        );

        // Omitted → empty list (no efficiency middleware; the governed gate still applies).
        let bare: EngineSpec = serde_json::from_value(json!({
            "graph": minimal_graph_json(),
            "agents": { "assistant": { "provider": "anthropic" } }
        }))
        .expect("spec parses");
        assert!(bare
            .agents
            .get("assistant")
            .expect("agent present")
            .resolved_middleware
            .is_empty());
    }

    #[test]
    fn deserializes_an_agent_tier_from_camel_case() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "agents": {
                "assistant": { "provider": "mistral", "tier": "fast" }
            }
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        let agent = spec.agents.get("assistant").expect("agent present");
        assert_eq!(agent.tier, Some(ModelTier::Fast));
        assert!(agent.model.is_none());
    }

    #[test]
    fn subgraphs_default_to_empty_and_round_trip_child_definitions() {
        // Omitted → empty (an ordinary graph with no subgraph nodes).
        let spec: EngineSpec =
            serde_json::from_value(json!({ "graph": minimal_graph_json() })).expect("spec parses");
        assert!(spec.subgraphs.is_empty());

        // Present → each child GraphDefinition is carried verbatim, keyed elsewhere by
        // its own id. The bridge flattens child node ids into `jsNodeIds`/`agents`.
        let child = json!({
            "id": "child",
            "version": "0.0.0",
            "name": "child",
            "channels": {},
            "nodes": [{ "id": "c1", "type": "action", "label": "c1" }],
            "edges": [],
            "entryNodeId": "c1"
        });
        let spec: EngineSpec = serde_json::from_value(json!({
            "graph": minimal_graph_json(),
            "subgraphs": [child],
            "jsNodeIds": ["a", "c1"]
        }))
        .expect("spec parses");
        assert_eq!(spec.subgraphs.len(), 1);
        assert_eq!(spec.subgraphs[0].id.0, "child");
        assert_eq!(spec.js_node_ids, vec!["a".to_owned(), "c1".to_owned()]);
    }

    #[test]
    fn deserializes_a_component_node_spec() {
        let spec_json = json!({
            "graph": minimal_graph_json(),
            "componentNodes": {
                "a": {
                    "kind": "promptBuilder",
                    "params": { "template": "Hi {{name}}", "into": "prompt" }
                }
            }
        });
        let spec: EngineSpec = serde_json::from_value(spec_json).expect("spec parses");
        let component = spec.component_nodes.get("a").expect("component present");
        assert_eq!(component.kind, "promptBuilder");
        assert_eq!(
            component.params.get("into").and_then(Value::as_str),
            Some("prompt")
        );
    }

    #[test]
    fn run_outcome_serializes_camel_case() {
        let state = GraphState {
            run_id: ailu_graph_core::RunId::from("r"),
            graph_id: ailu_graph_core::GraphId::from("g"),
            current_node_id: ailu_graph_core::NodeId::from("a"),
            status: ailu_graph_core::GraphStatus::Suspended,
            channels: BTreeMap::new(),
            version: 1,
            checkpoint_id: Some("r:0".to_owned()),
            created_at: "0".to_owned(),
            updated_at: "0".to_owned(),
        };
        let outcome = RunOutcome {
            state,
            status: "suspended".to_owned(),
            pending_approvals: vec![ApprovalRequestItem {
                subject: "tool:refund".to_owned(),
                reason: "needs approval".to_owned(),
                approval_key: None,
                input: None,
            }],
            replay_journal: None,
            entry_state: None,
        };
        let wire = serde_json::to_string(&outcome).expect("serializes");
        assert!(wire.contains("\"pendingApprovals\""));
        assert!(wire.contains("\"tool:refund\""));
        assert!(wire.contains("\"currentNodeId\":\"a\""));
    }
}
