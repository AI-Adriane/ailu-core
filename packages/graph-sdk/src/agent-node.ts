import { ReActAgent, type AgentId, type ToolRegistry } from "@ailu-ai/agents-core";
import {
  InMemoryPromptRegistry,
  ModelPolicy,
  type LLMGateway,
  type LLMProvider,
  type ModelTier,
  type PromptRegistry
} from "@ailu-ai/llm-gateway";
import { toModelSpec, type ModelLike, type ModelSpec } from "@ailu-ai/model-core";
import { createToolNode, DynamicInterrupt, type NodeHandler } from "@ailu-ai/graph-runtime";
// Type-only: keeps the ApprovalEngine contract without pulling its Pg/db implementation
// (and a `pg` dependency) into consumers such as the Studio bundle.
import type { ApprovalEngine, ApprovalId } from "@ailu-ai/approval-engine";
import type { NodeId, RunId } from "@ailu-ai/graph-core";
import { AiluSdkError, GovernanceMiddlewareRejectedError } from "./errors.js";

/** Default channel an agent node writes its {@link import("@ailu-ai/agents-core").AgentResult} into. */
export const DEFAULT_AGENT_OUTPUT_CHANNEL = "agentResult";

/**
 * Channel holding the names of tools whose human approval has been granted. The
 * control plane writes it (see `CompiledGraph.approveAndResume`) before resuming a
 * run that suspended for approval; the agent then executes those tools.
 */
export const APPROVED_TOOLS_CHANNEL = "__approvedTools";

/** Reason carried by the dynamic interrupt an agent node raises when it needs approval. */
export const AGENT_APPROVAL_INTERRUPT = "agent-approval-required";

/**
 * Channel holding the ApprovalEngine request ids created when a run suspends for
 * approval. On resume the node looks each up; the ones the engine reports as
 * `approved` unlock their tools — the engine is the source of truth, not a flag.
 */
export const APPROVAL_IDS_CHANNEL = "__approvalIds";

const TOOL_SUBJECT_PREFIX = "tool:";

/** Where an agent node gets its system prompt. */
export type AgentPromptSource =
  | { registry: PromptRegistry; id: string; version?: string }
  /** Inline convenience: the SDK registers this string and references it by id. */
  | { system: string };

/**
 * Governed long-term memory overlay for an agent node (ADR 0026 phase 11). `namespace` is the
 * tenant-scoped memory partition; `topK`/`recall` tune retrieval. The principal is sealed by the
 * engine. The OSS engine recalls/persists in-memory; the control plane swaps a Neo4j-backed store
 * (native vector index + entity graph) behind the same seam.
 */
export type MemoryConfig = {
  namespace: string;
  topK?: number;
  recall?: "vector" | "graph" | "both";
};

/**
 * Governed skills overlay for an agent node (ADR 0035 phase 12) — progressive disclosure for deep
 * agents. The engine selects skills (explicit `required` pins by `name@version` + advisory vector
 * top-k over descriptions, capped by `advisoryK`) from this `namespace` before the run and prepends
 * their bodies to the seed. The `namespace` is tenant-scoped (sealed by the engine); a skill that
 * grants capability (`requires`) stays withheld until granted. Applies to this agent AND its
 * `mapAgents`/`taskNode` sub-agents (same build path → deepagents parity). The OSS engine selects
 * from an in-memory registry; the control plane swaps a Postgres-backed store behind the same seam.
 */
export type SkillConfig = {
  namespace: string;
  /** Explicit `name@version` pins — the must-apply playbooks, always loaded (when granted). */
  required?: string[];
  /** Cap on advisory (vector-selected) skills. Default 3; 0 = pins only. */
  advisoryK?: number;
};

/**
 * One skill RECORD supplied by the control plane for a run (ADR 0049 B-3) — the store contents the
 * engine builds its run-scoped `SkillStore` from. Matches Rust `Skill` (camelCase). `description` is the
 * always-resident L1 index; `body` is the on-demand L2; `requires` are approval-gated capability keys.
 * `allowedTools`/`license`/`metadata` are open-standard SKILL.md fields (ADR 0065 D1) — stored and
 * round-tripped through import/export, but `allowedTools` is NOT enforced by `SkillMiddleware` yet
 * (distinct from `requires`, which is the actual capability gate).
 */
export type SkillRecord = {
  name: string;
  version: string;
  namespace: string;
  description: string;
  body?: string;
  requires?: string[];
  allowedTools?: string[];
  license?: string;
  metadata?: Record<string, unknown>;
  resources?: Array<{ kind?: string; ref: string }>;
};

/** Config for {@link GraphBuilder.agentNode}. */
export type AgentNodeConfig = {
  /**
   * @deprecated (ADR 0031) Optional + dead on the Rust path — the engine builds its own gateway
   * from the provider slug + env keys. Pass a {@link AgentNodeConfig.model} overlay
   * (`@ailu-ai/model-openai`, …) instead. Still consulted only by the removed TS fallback.
   */
  llm?: LLMGateway;
  prompt: AgentPromptSource;
  tools?: ToolRegistry;
  /** @deprecated (ADR 0031) Use a {@link AgentNodeConfig.model} overlay. */
  provider?: LLMProvider;
  /**
   * The model to run. Either a provider overlay from a per-model package (ADR 0031) —
   * `model: openai("gpt-4o")` / `new OpenAIModel("gpt-4o")` — or a bare model-id string
   * (legacy; pairs with {@link AgentNodeConfig.provider}). A `ModelLike` carries its own
   * provider/tier and wins over the flat `provider`/`tier` fields.
   */
  model?: string | ModelLike;
  /**
   * Abstract capability tier (`"frontier" | "balanced" | "fast" | "creative"`). When
   * set and no explicit {@link AgentNodeConfig.model} is given, the concrete model is
   * resolved by the {@link ModelPolicy}: on the Rust path the bridge resolves it from
   * the process env (so "I only have Mistral" maps every tier to the mistral column);
   * on the TS fallback path the SDK resolves it here against `availableFromEnv()` so
   * the agent runs on a consistent concrete provider+model. An explicit `model` (and
   * an explicit `provider`) always wins over the tier (the override stays `false`-
   * recommended in policy terms).
   */
  tier?: ModelTier;
  /**
   * A named {@link AgentProfile} (`"fast" | "frontier-careful" | "governed-deep"`, ADR 0025
   * phase 3d) that sets the model tier, efficiency middleware, and suspend/fs defaults in one
   * shot. Explicit fields (`tier`, `suspendForApproval`, `enableFs`, `middleware`, the flat
   * `outputStyle`/`contextBudget` knobs) always override the profile's defaults. The governed
   * layer (redaction, approval gate, fs policy) is identical regardless of profile.
   */
  profile?: AgentProfile;
  /**
   * Extra EFFICIENCY middleware to append (ADR 0025 phase 3d), e.g. `[{ kind: "compress" }]`.
   * Efficiency-only by type: governance kinds (redact / approval gate / fs policy) are
   * engine-injected and rejected here ({@link GovernanceMiddlewareRejectedError}), so an
   * ungoverned stack is unrepresentable. Merged after the profile + the flat knobs, with
   * an explicit entry of the same `kind` winning (dedup, last-writer).
   */
  middleware?: EfficiencyMiddlewareSpec[];
  maxIterations?: number;
  name?: string;
  description?: string;
  /** Channel that receives the agent's result. Defaults to {@link DEFAULT_AGENT_OUTPUT_CHANNEL}. */
  outputChannel?: string;
  /**
   * Token-efficiency (ADR 0014). `"terse"` appends a compact-output directive to the
   * system prompt — cuts output tokens on **prose** stages (lossy; not for code). Default off.
   */
  outputStyle?: "terse";
  /**
   * Cap (in chars) on the agent's seed message — the injected `Input: <input>\nState:
   * <state>` dump — to avoid re-feeding an unbounded channel map to every agent. The cap
   * covers the whole seed message (ADR 0014 intent), not the `State` portion alone. Default: no cap.
   */
  contextBudget?: number;
  /**
   * Durable channel the agent's `writeTodos` list is persisted into (ADR 0022/0023,
   * phase 1). When set and the agent has the `writeTodos` tool, the engine writes the
   * authoritative todo list here in the same checkpointed update as the result, so
   * downstream nodes can read the plan. Default: no durable sink (the list still
   * appears in the result). Conventionally {@link import("@ailu-ai/agents-core").TODOS_CHANNEL} (`"__todos"`).
   */
  todosChannel?: string;
  /**
   * Channel carrying this run's multimodal input (ADR 0030 phase 9e): a `ContentBlock[]`
   * value (`{ type: "image" | "audio" | "file", source }` + optional `{ type: "text" }`).
   * When set, the agent's seed message becomes multimodal — a text Input/State digest plus
   * the media blocks — and this channel is excluded from the stringified State (so binary
   * bytes are never re-fed as text). Seed the channel via the run's `initialData`. Default:
   * text-only seed.
   */
  inputBlocksChannel?: string;
  /** The only channels the agent is shown in its seed state (context isolation); default all. */
  visibleChannels?: string[];
  /**
   * Governed long-term memory (ADR 0026 phase 11). When set, the engine recalls from this
   * namespace before the run (vector) and persists the run's reasoning after, attributed. The
   * `namespace` is tenant-scoped (the control plane validates access) and the principal is
   * sealed by the engine — never user-routable. `topK`/`recall` tune retrieval quality.
   */
  memory?: MemoryConfig;
  /**
   * Governed skills — progressive disclosure (ADR 0035 phase 12). When set, the engine selects
   * skills (explicit `required` pins + advisory vector top-k) from this `namespace` before the run
   * and prepends their bodies to the seed. Capability-granting (`requires`) skills are withheld
   * until granted. Applies to `mapAgents`/`taskNode` sub-agents too (deepagents parity).
   */
  skills?: SkillConfig;
  /**
   * Opt this agent into the governed virtual filesystem tools (ADR 0024 phase 2b):
   * `read_file`/`ls`/`glob`/`grep`/`write_file`/`edit_file`/`delete_file`/`move_file`,
   * run-scoped over a versioned artifact store and enforced by the graph's
   * {@link GraphBuilder.fsPolicy} (fail-closed read-only by default). Default off.
   */
  enableFs?: boolean;
  /**
   * When true, the node suspends the whole run (a dynamic interrupt) the moment the
   * agent needs approval, instead of just flagging `requiresHumanReview`. Resume with
   * `CompiledGraph.approveAndResume(runId, { approvedTools })` to continue. Default false.
   */
  suspendForApproval?: boolean;
  /**
   * Route approvals through an {@link ApprovalEngine}: on suspend the node files a
   * request per gated tool; on resume it executes the tools the engine reports as
   * approved. The engine becomes the source of truth (a human resolves it out of
   * band) instead of the `__approvedTools` channel.
   */
  approvalEngine?: ApprovalEngine;
  label?: string;
};

/**
 * A signed-off agent profile (ADR 0025 phase 3d) — a named bundle that expands to a model
 * tier, an efficiency-middleware set, and suspend/fs defaults. The GOVERNED layer (PII
 * redaction, the approval gate, fs policy) is identical across all profiles — you cannot
 * "buy out" of governance. Explicit `tier`/`suspendForApproval`/`enableFs`/`middleware` on
 * the config always win over the profile's defaults.
 *
 * - `fast` — `fast` tier, full efficiency (compress + terse + tight 4k context budget), no
 *   suspend. For high-throughput, low-stakes prose work.
 * - `frontier-careful` — `frontier` tier, NO compression (preserve fidelity), a roomy 16k
 *   budget, reflection, suspend-on-approval. For high-stakes reasoning where lossy compression
 *   is unsafe.
 * - `governed-deep` — the deep-agent one-liner: `balanced` tier, full efficiency (12k budget),
 *   reflection, suspend-on-approval, and the governed virtual filesystem enabled.
 */
export type AgentProfile = "fast" | "frontier-careful" | "governed-deep";

/**
 * An EFFICIENCY / quality middleware a user may append to an agent (ADR 0025 phase 3d/3e). The
 * governance kinds (`redact` / `approvalGate` / `fsPolicy`) are deliberately NOT part of this
 * union — they are engine-injected and sealed, so a user cannot express an ungoverned stack
 * (the governed-by-construction invariant). `retry` / `rateLimit` stay deferred (retry belongs
 * at the gateway; a rate-limit delay would conflict with the engine's determinism).
 *
 * - `compress` — route messages through the prompt-compression service (no-op if unconfigured).
 * - `terse` — append a compact-output directive to the system prompt (lossy; prose only).
 * - `contextBudget` — cap the agent's seed message (the injected `Input`/`State` dump) to `chars` characters.
 * - `reflection` — one self-critique after the run (ADR 0025 phase 3e): a weak result is flagged in
 *   the reasoning (`reflection:needs_review:<issues>`) for observability / downstream routing (a
 *   conditional edge can gate on it). It does NOT set `requiresHumanReview` — that would re-suspend
 *   forever on resume. Additive — the full critique→revise loop stays the standalone reflection
 *   node. `threshold` (0..1, default 0.8) is the acceptance bar.
 * - `structuredOutput` — constrain the agent's output to a JSON Schema (ADR 0029 phase 8). The
 *   engine sets the provider's native constraint (OpenAI `response_format`, Anthropic forced tool,
 *   Gemini `responseSchema`) AND validates the result in-engine (the floor). The validated value
 *   lands on `AgentResult.structuredOutput`. `mode: "required"` (default) fails closed with a typed
 *   error after `retryCap` (default 2) deterministic re-prompts; `mode: "lenient"` falls back to raw
 *   text. It is an EFFICIENCY kind (output-shaping), and the approval gate stays intrinsic — so it
 *   cannot route around governance.
 */
export type EfficiencyMiddlewareSpec =
  | { kind: "compress" }
  | { kind: "terse" }
  | { kind: "contextBudget"; params: { chars: number } }
  | { kind: "reflection"; params?: { threshold?: number } }
  | {
      kind: "structuredOutput";
      params: {
        schema: Record<string, unknown>;
        name?: string;
        strict?: boolean;
        mode?: "required" | "lenient";
        retryCap?: number;
      };
    };

/**
 * Governance middleware kinds the SDK rejects in {@link AgentNodeConfig.middleware}: they are
 * engine-injected (the GOVERNED layer), never user-supplied. Shared so the SDK throw-gate and
 * the contracts efficiency-only schema list the SAME kinds (they cannot drift). Passing one to
 * the builder throws `GovernanceMiddlewareRejectedError`. (The runtime enforcer on the Rust
 * side is independent: the bridge match only honours efficiency kinds and ignores these.)
 */
export const GOVERNANCE_MIDDLEWARE_KINDS = ["redact", "approvalGate", "fsPolicy"] as const;

/** A filesystem permission verb (ADR 0024): `deny` < `read` < `gate` < `write`. */
export type FsPermVerb = "deny" | "read" | "write" | "gate";

/**
 * A per-path filesystem permission rule (ADR 0024 phase 2b): a glob (`*` within a
 * path segment, `**` across) mapped to a verb. Compiled into the run's fail-closed
 * path policy ({@link GraphBuilder.fsPolicy}). An unmatched path resolves to `read`.
 */
export type FsPolicyRule = { glob: string; verb: FsPermVerb };

/**
 * Config for {@link GraphBuilder.mapAgents} — a dynamic fan-out (ADR 0027 phase 4b): run
 * `subAgent` once per item in the `overChannel` array, concurrently, and write the per-item
 * results — in input order (deterministic) — into `joinAt` as an array. Each spawn gets one item
 * as its input and shares the run's channels.
 */
export type MapAgentNodeConfig = {
  /** Channel holding the array of items to map the sub-agent over. */
  overChannel: string;
  /** The sub-agent to run per item (its own ReAct agent config). */
  subAgent: AgentNodeConfig;
  /** Channel the array of per-item results lands in (one entry per item, in input order). */
  joinAt: string;
  /** When true, a spawn that needs approval suspends the whole map (default false). */
  suspendForApproval?: boolean;
  label?: string;
};

/** The wire projection of a {@link MapAgentNodeConfig} the Rust bridge consumes (`map_agents`). */
export type RustMapAgentConfig = {
  overChannel: string;
  joinAt: string;
  agent: RustAgentConfig;
  suspendForApproval: boolean;
};

/** Config for {@link GraphBuilder.toolNode}. */
export type ToolNodeConfig = {
  tools: ToolRegistry;
  /** Execute all tool calls concurrently instead of sequentially. */
  parallel?: boolean;
  label?: string;
};

/**
 * Config for {@link GraphBuilder.taskNode} — spawn a sub-agent in an isolated context
 * that returns a single compressed report (ADR 0022/0023, phase 1). The spawn is sugar
 * over a one-node subgraph, so it inherits the runtime's guarantees: checkpointed,
 * audited, and human-gate-preserving (a sub-agent that suspends for approval suspends
 * the whole run).
 */
export type TaskNodeConfig = {
  /** The sub-agent to spawn (its own ReAct agent config). */
  subAgent: AgentNodeConfig;
  /**
   * The channel that feeds the sub-agent its objective — the ONLY channel projected
   * into the child (isolation). Default `"objective"`.
   */
  objectiveChannel?: string;
  /**
   * The channel the sub-agent's report lands in — the ONLY channel projected back to
   * the parent (a single value out). Default `"report"`.
   */
  reportChannel?: string;
  /**
   * Run the sub-agent with `outputStyle: "terse"` so the report is a summary, not a
   * full transcript. Default true.
   */
  compress?: boolean;
  label?: string;
};

/**
 * A tool's name plus its TS `execute` fn — the data the Rust bridge needs to back a
 * `jsToolName` with a JS callback. The Rust engine never imports the tool registry;
 * it calls this `execute` over the napi seam (`on_node` with `kind:"tool"`).
 */
export type RustToolBinding = {
  name: string;
  execute: (input: unknown) => Promise<unknown>;
};

/**
 * How one of an agent's tools is advertised to the LLM (→ Rust `AgentSpec.toolSpecs`): the tool's
 * `description` and the JSON Schema of its input (`ToolDefinition.jsonSchema`). Serializable, so
 * it also rides the persisted agent carrier.
 */
export type RustToolSpec = {
  name: string;
  description?: string;
  jsonSchema?: Record<string, unknown>;
};

/**
 * The serializable shape of an agent node, plus its JS-backed tool executes, that
 * the Rust engine bridge consumes (see `EngineSpec.agents` / `jsToolNames`). It is a
 * pure projection of {@link AgentNodeConfig} — the system prompt is the *resolved*
 * string (never a registry reference), since the bridge has no prompt registry.
 *
 * The LLM gateway itself is **not** carried: the Rust agent path builds its own
 * gateway (env adapters or a deterministic mock). A graph whose agents rely on a
 * specific TS `AgentNodeConfig.llm` therefore keeps its semantics only on the TS
 * engine; the Rust path is opt-in for agents (see `CompiledGraph`).
 */
export type RustAgentConfig = {
  provider: string;
  model?: string;
  /**
   * Abstract capability tier carried to the Rust `AgentSpec.tier`. When set with no
   * explicit `model`, the Rust bridge resolves the concrete model via `ModelPolicy`
   * against the process env. An explicit `model` always wins.
   */
  tier?: ModelTier;
  /**
   * A custom OpenAI-compatible endpoint (`model.openaiCompatible({ baseURL })`). The Rust bridge
   * serves the agent through the OpenAI-wire adapter at this URL, with the key read only from
   * {@link apiKeyEnv} (never `OPENAI_API_KEY`).
   */
  baseURL?: string;
  /** Env var holding the key for {@link baseURL}; unset means a keyless endpoint. */
  apiKeyEnv?: string;
  /** Resolved system prompt string. */
  system?: string;
  toolNames: string[];
  /** What the LLM is told about each tool: its description and input JSON Schema. */
  toolSpecs?: RustToolSpec[];
  maxIterations?: number;
  suspendForApproval: boolean;
  /** Tools (by name) requiring approval — those marked `requiresApproval`. */
  approvalToolNames: string[];
  outputChannel: string;
  /** ADR 0014 — terse output directive on the system prompt. */
  outputStyle?: "terse";
  /** ADR 0014 — cap (chars) on the injected seed message (the `Input`/`State` dump). */
  contextBudget?: number;
  /** ADR 0022/0023 — durable channel the `writeTodos` list is persisted into. */
  todosChannel?: string;
  /** ADR 0030 phase 9e — channel carrying the run's multimodal input blocks. */
  inputBlocksChannel?: string;
  /** The only channels the agent is shown in its seed state (context isolation); default all. */
  visibleChannels?: string[];
  /** ADR 0026 phase 11 — governed long-term memory overlay. */
  memory?: MemoryConfig;
  /** ADR 0035 phase 12 — governed skills (progressive disclosure) overlay. */
  skills?: SkillConfig;
  /** ADR 0024 phase 2b — opt this agent into the governed virtual filesystem tools. */
  enableFs?: boolean;
  /**
   * ADR 0025 phase 3d — the SDK-resolved EFFICIENCY middleware list: the profile + explicit
   * `middleware[]` + the legacy `outputStyle`/`contextBudget` knobs expanded into one ordered
   * list of `{ kind, params }` data entries the Rust bridge turns into `push_efficiency`
   * calls. The governed layer is never carried here (the bridge injects it). Always present
   * (possibly empty) on the live builder path; absent on a pre-3d persisted carrier (the Rust
   * bridge then falls back to the legacy flat knobs).
   */
  resolvedMiddleware?: EfficiencyMiddlewareSpec[];
  /** JS-backed tool executes, one per tool in the registry. */
  toolBindings: RustToolBinding[];
  /**
   * SDK-only (never serialized to the wire): whether this agent node was configured
   * with a TS {@link ApprovalEngine}. The engine-backed approval flow — filing a
   * request per gated tool and reading the engine's decision on resume — lives in the
   * TS `createAgentNodeHandler`; the Rust agent path does not invoke it. So a graph
   * with an `approvalEngine` keeps its agent nodes on the TS engine under `auto`.
   */
  usesApprovalEngine: boolean;
};

/**
 * The governance binding an agent node contributes to {@link CompiledGraph}: the
 * (optional) {@link ApprovalEngine} a human resolves requests through, the principal
 * that *requests* approvals on this node's behalf (`config.name ?? nodeId`, the same
 * `requestedBy` the node files requests under), and the names of its approval-gated
 * tools. {@link CompiledGraph.approveAndResume} uses it to (a) approve the matching
 * pending engine requests before resuming on the TS path, and (b) stamp each granted
 * tool's `requestedBy` for the Rust engine's no-self-approval guard-rail.
 */
export type AgentApprovalBinding = {
  approvalEngine?: ApprovalEngine;
  requestedBy: string;
  approvalToolNames: string[];
};

/** Project an {@link AgentNodeConfig} into its {@link AgentApprovalBinding}. */
export const toAgentApprovalBinding = (
  nodeId: string,
  config: AgentNodeConfig
): AgentApprovalBinding => ({
  approvalEngine: config.approvalEngine,
  requestedBy: config.name ?? nodeId,
  approvalToolNames: approvalToolNamesOf(config.tools)
});

/** Pull every tool's name + `execute` out of a registry, for the Rust tool seam. */
const toolBindingsOf = (tools: ToolRegistry | undefined): RustToolBinding[] => {
  if (tools === undefined) {
    return [];
  }
  return tools.list().map((definition) => {
    const resolved = tools.resolve(definition.id);
    const execute = resolved?.handler ?? (async () => ({}));
    return { name: definition.name, execute: (input: unknown) => execute(input) };
  });
};

/** Each tool's description and input JSON Schema, as the LLM should see them. */
const toolSpecsOf = (tools: ToolRegistry | undefined): RustToolSpec[] =>
  tools?.list().map((definition) => ({
    name: definition.name,
    description: definition.description,
    jsonSchema: definition.jsonSchema
  })) ?? [];

/** Tool names whose definition is flagged `requiresApproval`. */
const approvalToolNamesOf = (tools: ToolRegistry | undefined): string[] => {
  if (tools === undefined) {
    return [];
  }
  return tools
    .list()
    .filter((definition) => definition.requiresApproval === true)
    .map((definition) => definition.name);
};

/**
 * The signed-off {@link AgentProfile} expansion table (ADR 0025 phase 3d). Each profile is a
 * named data bundle of a model tier, suspend/fs defaults, and an ordered efficiency-middleware
 * list. The governed layer is identical across all three. `governed-deep → balanced` is the
 * ADR-literal tier (decision confirmed 2026-06-23). Reflection is ADR phase 3e, so it is not
 * yet part of any profile's middleware.
 */
const PROFILES: Record<
  AgentProfile,
  {
    tier: ModelTier;
    suspendForApproval: boolean;
    enableFs: boolean;
    middleware: EfficiencyMiddlewareSpec[];
  }
> = {
  fast: {
    tier: "fast",
    suspendForApproval: false,
    enableFs: false,
    middleware: [{ kind: "compress" }, { kind: "terse" }, { kind: "contextBudget", params: { chars: 4000 } }]
  },
  "frontier-careful": {
    tier: "frontier",
    suspendForApproval: true,
    enableFs: false,
    // No compression — lossy compression is unsafe for high-stakes reasoning. Reflection
    // escalates a weak answer to human review (ADR 0025 phase 3e).
    middleware: [{ kind: "contextBudget", params: { chars: 16000 } }, { kind: "reflection" }]
  },
  "governed-deep": {
    tier: "balanced",
    suspendForApproval: true,
    enableFs: true,
    middleware: [
      { kind: "compress" },
      { kind: "terse" },
      { kind: "contextBudget", params: { chars: 12000 } },
      { kind: "reflection" }
    ]
  }
};

/**
 * Resolve suspend-on-approval, honouring the {@link AgentProfile} default when the config
 * leaves it unset (an explicit `suspendForApproval`, including `false`, always wins). Shared
 * by {@link toRustAgentConfig} (the Rust/persisted path) and {@link createAgentNodeHandler}
 * (the TS handler) so the two cannot disagree on a profile's human-gate default.
 */
const resolveSuspendForApproval = (
  config: Pick<AgentNodeConfig, "suspendForApproval" | "profile">
): boolean =>
  config.suspendForApproval ??
  (config.profile !== undefined ? PROFILES[config.profile].suspendForApproval : false);

/**
 * Desugar an agent config's {@link AgentProfile} + flat `outputStyle`/`contextBudget` knobs +
 * explicit {@link AgentNodeConfig.middleware} into ONE ordered EFFICIENCY-middleware list (ADR
 * 0025 phase 3d). Precedence (most specific wins): profile → flat knobs → explicit middleware,
 * deduped by `kind` keeping the last writer. Throws {@link GovernanceMiddlewareRejectedError}
 * if an explicit entry names a governance kind — the SDK's authoritative live-path reject gate.
 */
const resolveMiddleware = (config: AgentNodeConfig): EfficiencyMiddlewareSpec[] => {
  // Insertion order is preserved by Map; re-`set`ting a kind updates the value in place
  // (last-writer-wins) without changing its position — order among efficiency hooks is
  // immaterial (they act on disjoint parts of the request).
  const byKind = new Map<string, EfficiencyMiddlewareSpec>();
  if (config.profile !== undefined) {
    for (const middleware of PROFILES[config.profile].middleware) {
      byKind.set(middleware.kind, middleware);
    }
  }
  if (config.outputStyle === "terse") {
    byKind.set("terse", { kind: "terse" });
  }
  if (typeof config.contextBudget === "number") {
    byKind.set("contextBudget", { kind: "contextBudget", params: { chars: config.contextBudget } });
  }
  for (const middleware of config.middleware ?? []) {
    if ((GOVERNANCE_MIDDLEWARE_KINDS as readonly string[]).includes(middleware.kind)) {
      throw new GovernanceMiddlewareRejectedError(middleware.kind);
    }
    byKind.set(middleware.kind, middleware);
  }
  return [...byKind.values()];
};

/**
 * Project an {@link AgentNodeConfig} into the {@link RustAgentConfig} the Rust engine
 * bridge consumes. Resolves the system prompt to a concrete string, pulls the tool
 * names / approval flags / executes out of the registry, and desugars the profile +
 * middleware into a {@link RustAgentConfig.resolvedMiddleware} list. Pure — no LLM call.
 */
export const toRustAgentConfig = (nodeId: string, config: AgentNodeConfig): RustAgentConfig => {
  const { registry, id, version } = resolvePrompt(nodeId, config.prompt);
  let system: string | undefined;
  try {
    system = registry.get(id, version).system;
  } catch {
    system = undefined;
  }
  // A profile supplies tier / suspend / fs defaults; an explicit field always wins.
  const profile = config.profile !== undefined ? PROFILES[config.profile] : undefined;
  // ADR 0031: a `model` overlay (ModelSpec/Model) carries its own provider/model/tier and wins
  // over the flat provider/model/tier aliases. A `"provider:model"` string is parsed like
  // `model("provider:model")`; a bare string without a provider stays a legacy model id.
  const spec = toAgentModelSpec(config.model);
  const modelId = spec !== undefined ? spec.model : typeof config.model === "string" ? config.model : undefined;
  return {
    provider: spec?.provider ?? config.provider ?? "anthropic",
    model: modelId,
    tier: spec?.tier ?? config.tier ?? profile?.tier,
    baseURL: spec?.baseURL,
    apiKeyEnv: spec?.apiKeyEnv,
    system,
    toolNames: config.tools?.list().map((definition) => definition.name) ?? [],
    toolSpecs: toolSpecsOf(config.tools),
    maxIterations: config.maxIterations,
    suspendForApproval: resolveSuspendForApproval(config),
    approvalToolNames: approvalToolNamesOf(config.tools),
    outputChannel: config.outputChannel ?? DEFAULT_AGENT_OUTPUT_CHANNEL,
    outputStyle: config.outputStyle,
    contextBudget: config.contextBudget,
    todosChannel: config.todosChannel,
    inputBlocksChannel: config.inputBlocksChannel,
    visibleChannels: config.visibleChannels,
    memory: config.memory,
    skills: config.skills,
    enableFs: config.enableFs ?? profile?.enableFs,
    resolvedMiddleware: resolveMiddleware(config),
    toolBindings: toolBindingsOf(config.tools),
    usesApprovalEngine: config.approvalEngine !== undefined
  };
};

/**
 * The {@link ModelSpec} an agent's `model` field declares: a model overlay, or a
 * `"provider:model"` / `"provider:tier"` string (an unknown provider fails loud). `undefined` for
 * no model or a bare legacy model id.
 */
const toAgentModelSpec = (value: AgentNodeConfig["model"]): ModelSpec | undefined => {
  if (value === undefined) return undefined;
  if (typeof value === "object") return toModelSpec(value);
  return value.includes(":") ? toModelSpec(value) : undefined;
};

/**
 * The serializable agent carrier persisted on `node.metadata.agent` (and on a `mapAgents`
 * node's `subAgent`): every wire field of the {@link RustAgentConfig}, never the tool closures,
 * so the saved GraphDefinition runs the same on the catalog path (`runCatalogGraph`).
 */
export const toAgentCarrier = (config: RustAgentConfig): Record<string, unknown> => ({
  provider: config.provider,
  model: config.model,
  tier: config.tier,
  baseURL: config.baseURL,
  apiKeyEnv: config.apiKeyEnv,
  system: config.system,
  toolNames: config.toolNames,
  toolSpecs: config.toolSpecs,
  maxIterations: config.maxIterations,
  suspendForApproval: config.suspendForApproval,
  approvalToolNames: config.approvalToolNames,
  outputChannel: config.outputChannel,
  outputStyle: config.outputStyle,
  contextBudget: config.contextBudget,
  todosChannel: config.todosChannel,
  inputBlocksChannel: config.inputBlocksChannel,
  visibleChannels: config.visibleChannels,
  memory: config.memory,
  skills: config.skills,
  enableFs: config.enableFs,
  resolvedMiddleware: config.resolvedMiddleware
});

const resolvePrompt = (
  nodeId: string,
  prompt: AgentPromptSource
): { registry: PromptRegistry; id: string; version?: string } => {
  if ("system" in prompt) {
    // Even inline prompts are referenced by id, never hardcoded into the agent —
    // we register the string under a deterministic id and hand back a reference.
    const registry = new InMemoryPromptRegistry();
    const id = `sdk.agent.${nodeId}.system`;
    registry.register({ id, version: "1.0.0", system: prompt.system });
    return { registry, id, version: "1.0.0" };
  }
  return { registry: prompt.registry, id: prompt.id, version: prompt.version };
};

/** Config for {@link streamAgentTokens}. */
export type StreamAgentConfig = {
  llm: LLMGateway;
  prompt: AgentPromptSource;
  provider?: LLMProvider;
  model?: string;
};

/**
 * Stream an agent's reply token by token through the gateway's `stream()`. This is
 * the single-turn (no-tools) path — ideal for a chat UI that wants live output.
 * Yields text deltas as they arrive and returns when the provider signals done.
 *
 * ```ts
 * for await (const delta of streamAgentTokens({ llm, prompt: { system } }, "Bonjour ?")) {
 *   process.stdout.write(delta);
 * }
 * ```
 */
export async function* streamAgentTokens(config: StreamAgentConfig, input: unknown): AsyncIterable<string> {
  const { registry, id, version } = resolvePrompt("stream", config.prompt);
  const system = registry.get(id, version).system;

  const stream = config.llm.stream({
    provider: config.provider ?? "anthropic",
    model: config.model ?? "claude-opus-4-8",
    system,
    messages: [{ role: "user", content: typeof input === "string" ? input : JSON.stringify(input) }]
  });

  for await (const chunk of stream) {
    if (chunk.delta.length > 0) {
      yield chunk.delta;
    }
    if (chunk.done) {
      return;
    }
  }
}

/**
 * Build the handler for an agent node: a {@link ReActAgent} driven by the given
 * LLM gateway. The agent's result is written to `outputChannel`; route on its
 * `requiresHumanReview` flag (e.g. a conditional edge into a human gate) to keep
 * sensitive actions behind approval — an agent never self-approves.
 */
const channelArray = (channels: Record<string, unknown>, key: string): string[] => {
  const value = channels[key];
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
};

/** A tool subject is `{ description: "tool:<name>" }`; pull the tool name back out. */
const subjectToolName = (subject: { description: string } | { [key: string]: unknown }): string | undefined => {
  const description = (subject as { description?: unknown }).description;
  return typeof description === "string" && description.startsWith(TOOL_SUBJECT_PREFIX)
    ? description.slice(TOOL_SUBJECT_PREFIX.length)
    : undefined;
};

/**
 * The set of approval-gated tools the agent may now run. The channel path covers
 * `approveAndResume`; the engine path covers a real {@link ApprovalEngine} decision
 * resolved out of band — we look up the request ids stashed at suspend time.
 */
const resolveApprovedTools = async (
  channels: Record<string, unknown>,
  engine: ApprovalEngine | undefined
): Promise<string[]> => {
  const approved = new Set(channelArray(channels, APPROVED_TOOLS_CHANNEL));
  if (engine !== undefined) {
    for (const rawId of channelArray(channels, APPROVAL_IDS_CHANNEL)) {
      const request = await engine.getById(rawId as ApprovalId);
      if (request?.status === "approved") {
        const toolName = subjectToolName(request.subject);
        if (toolName !== undefined) {
          approved.add(toolName);
        }
      }
    }
  }
  return [...approved];
};

/**
 * Resolve the concrete `{ provider, model }` an agent node runs on, honouring the
 * explicit-override precedence: an explicit `model`/`provider` always wins; a `tier`
 * (with no explicit model) maps through the {@link ModelPolicy} against the providers
 * available in the current env. This keeps the TS fallback path consistent with the
 * Rust bridge's `resolve_agent_model` — "I only have Mistral" resolves every tier to
 * the mistral column. With neither tier nor a usable provider, returns the config's
 * explicit values (the {@link ReActAgent} then applies its own defaults).
 */
export const resolveAgentModel = (
  config: Pick<AgentNodeConfig, "provider" | "model" | "tier">
): { provider?: LLMProvider; model?: string } => {
  // ADR 0031: normalize a `model` overlay (ModelSpec/Model) or a legacy string to a model id +
  // tier. (This is the removed TS-fallback resolver; the overlay's provider is honoured on the
  // Rust path in toRustAgentConfig — here only the flat provider feeds the legacy ModelPolicy.)
  const spec = toAgentModelSpec(config.model);
  const modelId = spec !== undefined ? spec.model : typeof config.model === "string" ? config.model : undefined;
  const tier = spec?.tier ?? config.tier;
  // No tier, or an explicit model already pins the choice: keep what was given so the
  // explicit override wins and the ReActAgent default applies when unset.
  if (tier === undefined || modelId !== undefined) {
    return { provider: config.provider, model: modelId };
  }
  const policy = new ModelPolicy();
  const available = policy.availableFromEnv();
  const choice = policy.resolve(tier, available, { provider: config.provider });
  return { provider: choice.provider, model: choice.model };
};

export const createAgentNodeHandler = (nodeId: string, config: AgentNodeConfig): NodeHandler => {
  const { registry, id, version } = resolvePrompt(nodeId, config.prompt);
  const outputChannel = config.outputChannel ?? DEFAULT_AGENT_OUTPUT_CHANNEL;
  const { provider, model } = resolveAgentModel(config);

  return async (input, state, context) => {
    // ADR 0016/0031: this is the removed TS-fallback handler — the Rust engine runs agents
    // natively (it never invokes this), so building it must NOT require `llm`. Only if a run
    // actually routes here (the dead TS path) is a gateway required. Checked at call time so
    // `agentNode({ model })` (no llm) builds cleanly for the Rust path.
    const llm = config.llm;
    if (llm === undefined) {
      throw new AiluSdkError(
        "This run reached the legacy TS-fallback agent handler, which needs `llm`. On the Rust " +
          "engine agents run natively — declare a `model` overlay (e.g. model.openai('gpt-4o')).",
        {
          code: "AILU_LEGACY_TS_AGENT_HANDLER",
          hint: "Give the agent node a `model` (e.g. model.openai('gpt-4o')) so it runs natively on the Rust engine; the `llm` gateway was only for the removed TS fallback."
        }
      );
    }
    const channels = state.channels as Record<string, unknown>;
    const approvedToolNames = await resolveApprovedTools(channels, config.approvalEngine);

    const agent = new ReActAgent<unknown>({
      id: nodeId as AgentId,
      name: config.name ?? nodeId,
      description: config.description ?? `agent node ${nodeId}`,
      llm,
      tools: config.tools,
      provider,
      model,
      maxIterations: config.maxIterations,
      promptRegistry: registry,
      promptId: id,
      promptVersion: version,
      approvedToolNames
    });

    const result = await agent.run(input, state, {
      memory: context.memory,
      workingMemory: { shortTerm: [], longTerm: context.memory }
    });

    // Native suspend-on-approval: stop the whole run cleanly (a checkpointed,
    // resumable suspension) rather than leaving routing to the caller. The pending
    // result — including its approvalRequests — is persisted to the output channel.
    // Honour the profile's suspend default (shared with toRustAgentConfig so the TS
    // handler and the Rust/persisted path agree on a profile's human-gate default).
    if (resolveSuspendForApproval(config) && result.requiresHumanReview) {
      const patch: Record<string, unknown> = { [outputChannel]: result };

      // File one ApprovalEngine request per gated tool and stash the ids, so resume
      // can ask the engine which were approved. The agent is the requester — a human
      // (a different principal) resolves it, which the engine enforces.
      if (config.approvalEngine !== undefined) {
        const ids: string[] = [];
        for (const request of result.approvalRequests) {
          const created = await config.approvalEngine.request({
            runId: state.runId as RunId,
            nodeId: state.currentNodeId as NodeId,
            requestedBy: config.name ?? nodeId,
            subject: request.subject
          });
          ids.push(String(created.id));
        }
        patch[APPROVAL_IDS_CHANNEL] = ids;
      }

      throw new DynamicInterrupt(AGENT_APPROVAL_INTERRUPT, patch);
    }

    return { [outputChannel]: result };
  };
};

/**
 * Build the handler for a tool node: executes the tool calls emitted by the last
 * AI message in the `messages` channel. Tools flagged `requiresApproval` suspend
 * the run via a dynamic interrupt instead of executing.
 */
export const createToolNodeHandler = (config: ToolNodeConfig): NodeHandler =>
  createToolNode(config.tools, { parallel: config.parallel });
