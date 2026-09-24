/**
 * Run a **catalog graph** on the Rust engine.
 *
 * A catalog graph is a plain {@link GraphDefinition} (e.g. one authored in the Studio
 * graph editor, persisted as data, with no in-process TS handlers) whose nodes carry
 * the SHARED CARRIER in `node.metadata`:
 *
 *   - a COMPONENT node carries `node.metadata.component = { kind, params }`
 *   - an AGENT node carries `node.metadata.agent = { provider?, model?, tier?, system?,
 *     toolNames?, maxIterations?, suspendForApproval?, approvalToolNames?, outputChannel? }`
 *
 * This is the seam the control plane (`apps/api`) uses to EXECUTE a graph built from
 * the catalog: it reads each node's metadata, assembles the engine's
 * `EngineSpec.componentNodes` + `agents` maps + the `jsNodeIds` for plain
 * action/tool nodes, and drives the run on the **Rust engine** via `@ailu-ai/napi`.
 *
 * Unlike {@link import("./builder.js").GraphBuilder}, there are no TS handler closures
 * here — components and agents run **natively** in Rust, and plain action/tool nodes
 * are inert JS seams (they return an empty channel update). The carrier IS the wiring.
 *
 * The carrier readers below mirror the canonical Zod schema in
 * `@ailu-ai/contracts` (`node-metadata.ts`); the SDK stays dependency-free of the
 * contracts package, so the narrowing is duplicated structurally here. The control
 * plane is free to validate the carrier with the contracts schema before handing the
 * definition to this runner.
 */

import type { GraphDefinition, GraphState, NodeId, RunId } from "@ailu-ai/graph-core";
import type { RunEvent } from "@ailu-ai/graph-runtime";
import type { ModelTier } from "@ailu-ai/llm-gateway";
// Type-only: keeps the ApprovalEngine contract without pulling its Pg/db implementation
// (and a `pg` dependency) into consumers such as the Studio bundle.
import type { ApprovalEngine } from "@ailu-ai/approval-engine";

import type {
  EfficiencyMiddlewareSpec,
  FsPolicyRule,
  RustAgentConfig,
  RustMapAgentConfig,
  RustToolBinding,
  RustToolSpec,
  SkillRecord
} from "./agent-node.js";
import { APPROVAL_IDS_CHANNEL, DEFAULT_AGENT_OUTPUT_CHANNEL } from "./agent-node.js";

/** Mirrors the Rust bridge's `SUBGRAPH_RUNS_KEY` (`runtime.rs`) — `{ <nodeId>: <childRunId> }`. */
const SUBGRAPH_RUNS_CHANNEL = "__subgraphRuns";
/** Mirrors the Rust bridge's `SUBGRAPH_STATES_KEY` (`runtime.rs`) — `{ <childRunId>: <GraphState> }`. */
const SUBGRAPH_STATES_CHANNEL = "__subgraphStates";
import type { RustComponentConfig, ComponentKind } from "./components.js";
import {
  rustEngineAvailable,
  tryCreateRustRunner,
  type ApprovedToolWire,
  type RustRunnerParts
} from "./rust-engine.js";
import type { ChannelValues } from "./typed.js";

/** The component carrier on `node.metadata.component`. Mirrors the contracts schema. */
export type ComponentCarrier = {
  kind: string;
  params: Record<string, unknown>;
};

/** The agent carrier on `node.metadata.agent`. Mirrors the contracts schema. */
export type AgentCarrier = {
  provider?: string;
  model?: string;
  tier?: ModelTier;
  /** A custom OpenAI-compatible endpoint, and the env var naming its key (`model.openaiCompatible`). */
  baseURL?: string;
  apiKeyEnv?: string;
  system?: string;
  toolNames?: string[];
  /** Each tool's description + input JSON Schema, as the LLM sees them. */
  toolSpecs?: RustToolSpec[];
  maxIterations?: number;
  suspendForApproval?: boolean;
  approvalToolNames?: string[];
  outputChannel?: string;
  /** ADR 0014 — terse output directive on the system prompt. */
  outputStyle?: "terse";
  /** ADR 0014 — cap (chars) on the agent's seed message (the injected `Input`/`State` dump). */
  contextBudget?: number;
  /** ADR 0022/0023 — durable channel the agent's `writeTodos` list is persisted into. */
  todosChannel?: string;
  /** ADR 0030 phase 9e — channel carrying the run's multimodal input blocks. */
  inputBlocksChannel?: string;
  /** The only channels the agent is shown in its seed state (context isolation). */
  visibleChannels?: string[];
  /** ADR 0026 phase 11 — governed long-term memory overlay. */
  memory?: { namespace: string; topK?: number; recall?: "vector" | "graph" | "both" };
  /** ADR 0035 phase 12 — governed skills (progressive disclosure) overlay. */
  skills?: { namespace: string; required?: string[]; advisoryK?: number };
  /** ADR 0024 — opt this agent into the governed virtual filesystem tools. */
  enableFs?: boolean;
  /**
   * ADR 0075 (issue #566 G3) — attach an external MCP server's tools to this agent, mid-run. The
   * control plane resolves this to a connection, discovers the server's tools, and merges them into
   * `toolNames`/`approvalToolNames` before the run reaches this carrier — the Rust engine itself
   * never resolves an MCP connection.
   */
  mcpConnectionId?: string;
  /**
   * Issue #566 G19 — governed action-tool connectors, keyed by provider id ("slack" first).
   * Resolved the same way as `mcpConnectionId`; generalized to a map so future providers don't
   * need a new engine field/release each time.
   */
  actionConnections?: Record<string, string>;
  /**
   * ADR 0025 phase 3d — the resolved efficiency middleware list. Present on graphs built by
   * the phase-3d SDK; absent on a pre-3d persisted node (the Rust bridge then falls back to
   * the legacy `outputStyle`/`contextBudget` knobs above, so old graphs keep their behaviour).
   */
  resolvedMiddleware?: EfficiencyMiddlewareSpec[];
};

/**
 * The mapAgents carrier on `node.metadata.mapAgents` (ADR 0027 phase 4b — dynamic fan-out). Mirrors
 * the contracts schema: run `subAgent` once per item in `overChannel` and collect the per-item results
 * (input order) into `joinAt`. The sub-agent is a full agent carrier → skills/memory/fs/planning apply.
 */
export type MapAgentCarrier = {
  overChannel: string;
  joinAt: string;
  subAgent: AgentCarrier;
  suspendForApproval?: boolean;
};

/** Outcome of a catalog-graph run: the terminal/suspended state and a flat status. */
export type CatalogRunOutcome = {
  /** The final (or suspended) graph state, channels included. */
  state: GraphState;
  /** `"running" | "suspended" | "completed" | "failed" | "cancelled"` — the state's status.
   *  `"cancelled"` (ADR 0044) means the run stopped at a node boundary because `options.signal`
   *  was aborted; its last checkpoint is durable, so it stays resumable and replayable. */
  status: string;
  /** True when execution ran on the Rust engine (always, since this seam requires it). */
  usedRustEngine: true;
  /**
   * Replay-as-evidence (ADR 0038): the recorded LLM I/O + clock journal (`{ decisions, clock }`
   * JSON) when the run executed in record mode (`AILU_LLM_RECORD`); `undefined` otherwise. The
   * control plane persists it to re-feed a later replay (`verify-replay`).
   */
  replayJournal?: string;
  /**
   * Replay-as-evidence (ADR 0040): the run's ENTRY state (initial state, before the entry node ran),
   * surfaced only on a record-mode run; `undefined` otherwise. The control plane persists it as the
   * checkpoint a later `verify-replay` seeds {@link replayCatalogGraph} from.
   */
  entryState?: GraphState;
  /**
   * The pending approvals — the subjects the run requested when it suspended on a gate (empty if it
   * completed without gating). On a {@link replayCatalogGraph} this is what the deterministic
   * re-execution requested: the faithfulness signal `verify-replay` compares to the attested chain.
   */
  pendingApprovals?: { subject: string; reason: string; approvalKey?: string; input?: unknown }[];
};

/** Options for {@link runCatalogGraph} / {@link resumeCatalogGraph}. */
export type RunCatalogGraphOptions = {
  /** A stable run id. Defaults to a generated one. */
  runId?: RunId;
  /** Initial channel data seeding the run. */
  initialData?: Record<string, unknown>;
  /** Subscribe to forwarded run-lifecycle events (every node transition). */
  onEvent?: (event: RunEvent) => void;
  /**
   * Cooperative cancellation (ADR 0044) — the kill switch. Abort this signal and the engine
   * stops the run at the **next node boundary**: it finishes the node it is on, writes that
   * node's checkpoint, emits a `run_cancelled` event and returns a terminal
   * {@link CatalogRunOutcome} with `status: "cancelled"`.
   *
   * It is **cooperative, never pre-emptive**. A node already executing (an agent mid-LLM-call,
   * a tool mid-HTTP-request) always runs to completion — so cancelling can never tear state,
   * and the last checkpoint always remains authoritative. That also means cancellation is not
   * instantaneous: its latency is the duration of the node in flight. A caller that needs a
   * *bounded* stop must impose its own deadline around this call; the engine deliberately
   * offers no hard abort, because killing a run mid-node is precisely what would break the
   * checkpoint-per-node guarantee everything else depends on.
   *
   * An ALREADY-aborted signal stops the run before its first node executes. Omit for the
   * previous behaviour (a run that can only complete, suspend or fail).
   */
  signal?: AbortSignal;
  /**
   * Opt into per-token streaming (ADR 0033 phase 13 / ADR 0060). When true, an agent node's LLM call
   * streams real provider deltas, surfaced as `token_delta` {@link RunEvent}s over {@link onEvent} — so
   * a catalog run (e.g. Governed Ask) can stream its answer token-by-token, not just return a final
   * result. The assembled state is byte-identical either way (deltas bypass the checkpoint/journal —
   * observational only). Default false (the run returns its terminal state with no token events).
   */
  streamTokens?: boolean;
  /**
   * Route the run's approvals through an {@link ApprovalEngine}. When present, the
   * agents run natively on Rust as usual, but the moment the run suspends for approval
   * the seam files one request per gated tool (`requestedBy = nodeId`, the agent's own
   * subject) and stashes the engine ids in the `__approvalIds` channel of the returned
   * state — so a human resolves them out of band (the engine forbids self-approval) and
   * the control plane only ever resumes with engine-approved tools. Absent: the run is
   * ungoverned (the legacy channel-only behaviour).
   */
  approvalEngine?: ApprovalEngine;
  /**
   * Per-provider API keys injected by the control plane (ADR 0010), keyed by provider
   * slug (`openai`, `anthropic`, `mistral`, …). Threaded into the Rust `EngineSpec` so
   * the gateway resolves each agent's key tenant-key-first then host env. Omit to rely
   * purely on the host env (local dev, tests).
   */
  providerKeys?: Record<string, string>;
  /**
   * Per-path filesystem permission rules (ADR 0024 phase 2d) the control plane resolved
   * for this run (from its owner-only `fs_path_policy` table), compiled into the engine's
   * `EngineSpec.fsPolicy` and applied to every fs-enabled agent. Omit for fail-closed
   * read-only everywhere.
   */
  fsPolicy?: FsPolicyRule[];
  /**
   * The tenant's governed skills for this run (ADR 0049 B-3) — the control plane's skill store. The
   * engine builds a run-scoped, tenant-isolated store from these and each agent's SkillMiddleware
   * selects from it. Omit/empty → the OSS shared in-memory store (no skills).
   */
  skills?: SkillRecord[];
  /**
   * Host tools for this run (ADR 0041 D1): JS-backed `{ name, execute }` bindings made callable by
   * ANY catalog agent whose `toolNames` includes the name — through the same napi host-tool seam the
   * in-process builder path uses (`on_node` `kind:"tool"`). Names NOT bound here keep the no-op stub
   * behaviour, so a graph remains pure data and existing runs are untouched. Supplied per CALL, never
   * persisted with the graph. NOTE (ADR 0041 E2): until the replay journal records host-tool results,
   * the caller must not combine `tools` with record-mode replay evidence — a replayed run would
   * re-execute the tools and may diverge.
   */
  tools?: RustToolBinding[];
  /**
   * Child graph definitions for `subgraph`-type nodes (ADR 0042, product ADR 0068 — child
   * workflows). A catalog node with `type: "subgraph"` + `subgraphId` resolves against this list,
   * exactly like the in-process builder path (`GraphBuilder.subgraph()` → `CompiledGraph`) already
   * does — `execute_subgraph` (the Rust engine) recursively starts/resumes the child sharing this
   * run's checkpointer, propagates the child's suspension to the parent, and propagates a child
   * failure as the parent's own error. Omit/empty for a graph with no subgraph nodes (today's
   * behaviour, unchanged).
   */
  subgraphs?: GraphDefinition[];
};

/** Raised when the native engine is unavailable — catalog graphs require it. */
export class RustEngineUnavailableError extends Error {
  public constructor() {
    super(
      "Catalog graphs execute on the Rust engine, but the native addon (@ailu-ai/napi) " +
        "is not available. Build it with scripts/build-napi.sh."
    );
    this.name = "RustEngineUnavailableError";
  }
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Narrow a node's open metadata bag to its COMPONENT carrier, if present and valid. */
export const readComponentCarrier = (
  metadata: Record<string, unknown> | undefined
): ComponentCarrier | undefined => {
  const component = metadata?.component;
  if (!isRecord(component)) {
    return undefined;
  }
  const { kind, params } = component;
  if (typeof kind !== "string" || kind.length === 0) {
    return undefined;
  }
  return { kind, params: isRecord(params) ? params : {} };
};

/** Narrow a node's open metadata bag to its AGENT carrier, if present and valid. */
export const readAgentCarrier = (
  metadata: Record<string, unknown> | undefined
): AgentCarrier | undefined => {
  const agent = metadata?.agent;
  if (!isRecord(agent)) {
    return undefined;
  }
  return agent as AgentCarrier;
};

/** Narrow a node's open metadata bag to its mapAgents (dynamic fan-out) carrier, if present + valid. */
export const readMapAgentCarrier = (
  metadata: Record<string, unknown> | undefined
): MapAgentCarrier | undefined => {
  const map = metadata?.mapAgents;
  if (!isRecord(map)) {
    return undefined;
  }
  const { overChannel, joinAt, subAgent } = map;
  if (typeof overChannel !== "string" || overChannel.length === 0) return undefined;
  if (typeof joinAt !== "string" || joinAt.length === 0) return undefined;
  if (!isRecord(subAgent)) return undefined;
  return {
    overChannel,
    joinAt,
    subAgent: subAgent as AgentCarrier,
    suspendForApproval: map.suspendForApproval === true
  };
};

/**
 * Project an {@link AgentCarrier} into the wire {@link RustAgentConfig} the bridge
 * consumes. `usesApprovalEngine` reflects whether the run was given an
 * {@link ApprovalEngine}: on the catalog path the agent still executes natively on Rust
 * (the flag does not re-route it), but the run is governed — the seam files a request
 * per gated tool when the run suspends (see {@link fileApprovalRequests}).
 */
const carrierToAgentConfig = (
  carrier: AgentCarrier,
  usesApprovalEngine: boolean
): RustAgentConfig => ({
  provider: carrier.provider ?? "anthropic",
  model: carrier.model,
  tier: carrier.tier,
  baseURL: carrier.baseURL,
  apiKeyEnv: carrier.apiKeyEnv,
  system: carrier.system,
  toolNames: carrier.toolNames ?? [],
  toolSpecs: carrier.toolSpecs,
  maxIterations: carrier.maxIterations,
  suspendForApproval: carrier.suspendForApproval === true,
  approvalToolNames: carrier.approvalToolNames ?? [],
  outputChannel: carrier.outputChannel ?? DEFAULT_AGENT_OUTPUT_CHANNEL,
  // ADR 0014 token-efficiency knobs + ADR 0022/0023 durable todos channel: carried on
  // the persisted node so the catalog/Studio run path reaches parity with the in-process
  // SDK builder path (toRustAgentConfig), which forwards the same fields.
  outputStyle: carrier.outputStyle,
  contextBudget: carrier.contextBudget,
  todosChannel: carrier.todosChannel,
  inputBlocksChannel: carrier.inputBlocksChannel,
  visibleChannels: carrier.visibleChannels,
  memory: carrier.memory,
  skills: carrier.skills,
  // ADR 0024 — fs enablement carried on the persisted node; the run's fs policy is
  // supplied separately by the control plane (RunCatalogGraphOptions.fsPolicy).
  enableFs: carrier.enableFs,
  // ADR 0025 phase 3d — forward the resolved efficiency list (already desugared at build
  // time); a pre-3d carrier has none, and the Rust bridge falls back to the flat knobs.
  resolvedMiddleware: carrier.resolvedMiddleware,
  // Per-agent tool closures are a builder-path concept; on the catalog path host tools are
  // supplied PER RUN via RunCatalogGraphOptions.tools (ADR 0041 D1) and dispatched through the
  // spec-level jsToolNames — an agent's toolName not bound there stays a native/no-op stub.
  toolBindings: [],
  usesApprovalEngine
});

const generateRunId = (): RunId => {
  const random = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
  return `run_${random}` as RunId;
};

/**
 * Assemble the {@link RustRunnerParts} for a catalog graph from its node-metadata
 * carriers. Component and agent nodes are routed to native Rust handlers; every other
 * non-human-gate node becomes an inert JS node (an empty channel update) so a graph
 * that mixes catalog nodes with plain action/tool nodes still runs end-to-end.
 */
const assembleParts = (
  definition: GraphDefinition,
  usesApprovalEngine: boolean,
  providerKeys: Record<string, string> | undefined,
  fsPolicy: FsPolicyRule[] | undefined,
  skills: SkillRecord[] | undefined,
  tools: RustToolBinding[] | undefined,
  subgraphs: GraphDefinition[] | undefined
): RustRunnerParts<ChannelValues> => {
  const components = new Map<string, RustComponentConfig>();
  const agents = new Map<string, RustAgentConfig>();
  const mapAgents = new Map<string, RustMapAgentConfig>();
  const jsNodeIds = new Set<string>();

  // Classifies ONE node into the shared maps above. Applied to the parent's own nodes AND
  // (ADR 0042) each subgraph's nodes — mirrors what GraphBuilder.subgraph() does at TS build
  // time on the builder path (flattening a child's handlers/agents/components into the SAME
  // maps as the parent, since `execute_subgraph` shares the parent runtime's registries; see
  // compiled-graph.ts's "child runs share these registries" comment). Without this, a subgraph's
  // OWN action/agent/component nodes would have no registered handler and the Rust bridge would
  // fail with "no node handler registered" the moment it tried to execute one.
  const classifyNode = (node: GraphDefinition["nodes"][number]): void => {
    const id = String(node.id);
    const component = readComponentCarrier(node.metadata);
    if (component !== undefined) {
      components.set(id, { kind: component.kind as ComponentKind, params: component.params });
      return;
    }
    // A mapAgents carrier takes precedence over a plain agent carrier (a fan-out node is not itself a
    // top-level agent) — the bridge routes it via EngineSpec.map_agents, keyed by node id.
    const mapAgent = readMapAgentCarrier(node.metadata);
    if (mapAgent !== undefined) {
      mapAgents.set(id, {
        overChannel: mapAgent.overChannel,
        joinAt: mapAgent.joinAt,
        agent: carrierToAgentConfig(mapAgent.subAgent, usesApprovalEngine),
        suspendForApproval: mapAgent.suspendForApproval === true
      });
      return;
    }
    // A node that CARRIES a mapAgents key but fails to parse (missing overChannel/joinAt/subAgent) would
    // otherwise fall through to an inert JS node and silently never fan out. Surface it — no silent caps.
    if (isRecord(node.metadata?.mapAgents)) {
      console.warn(
        `[ailu] node "${id}" has a malformed mapAgents carrier (needs overChannel, joinAt, subAgent) — it will NOT fan out.`
      );
    }
    const agent = readAgentCarrier(node.metadata);
    if (agent !== undefined) {
      agents.set(id, carrierToAgentConfig(agent, usesApprovalEngine));
      return;
    }
    if (node.type === "human-gate" || node.type === "subgraph") {
      // The runtime handles both natively (a human gate suspends; a nested subgraph recurses via
      // execute_subgraph, resolved against the SAME `subgraphs` list) — no JS handler needed.
      return;
    }
    // A plain action / tool / custom node with no carrier: an inert JS seam. The
    // catalog path has no TS handler closures, so it produces an empty update.
    jsNodeIds.add(id);
  };

  for (const node of definition.nodes) {
    classifyNode(node);
  }
  for (const subgraph of subgraphs ?? []) {
    for (const node of subgraph.nodes) {
      classifyNode(node);
    }
  }

  return {
    definition,
    // ADR 0042 (product ADR 0068 — child workflows): child graphs for `subgraph`-type nodes,
    // supplied per CALL like `tools`/`skills` — the same wire field the builder path
    // (`CompiledGraph`) already populates. Empty for a graph with no subgraph nodes.
    subgraphs: subgraphs ?? [],
    nodeFns: new Map(
      jsNodeIds.size === 0 ? [] : [...jsNodeIds].map((id) => [id, async () => ({})])
    ),
    // ADR 0041 D1 — per-run host tools: the same seam the builder path wires (`toolFns` backs the
    // napi `on_node` `kind:"tool"` dispatch; `jsToolNames` tells the bridge which names are real).
    // No bindings → today's behaviour exactly (every agent toolName is a native or no-op stub).
    toolFns: new Map((tools ?? []).map((binding) => [binding.name, binding.execute])),
    conditions: new Map(),
    agents,
    components,
    // ADR 0027 phase 4b / ADR 0049 — the catalog path now reads a `mapAgents` carrier (a dynamic
    // fan-out node), at parity with the in-process builder path.
    mapAgents,
    jsNodeIds,
    jsToolNames: new Set((tools ?? []).map((binding) => binding.name)),
    providerKeys,
    fsPolicy,
    skills
  };
};

/**
 * Run a catalog {@link GraphDefinition} (whose nodes carry `node.metadata.component`
 * and `node.metadata.agent`) to completion or suspension on the **Rust engine**.
 *
 * Throws {@link RustEngineUnavailableError} when the native addon is absent.
 */
export const runCatalogGraph = async (
  definition: GraphDefinition,
  options: RunCatalogGraphOptions = {}
): Promise<CatalogRunOutcome> => {
  if (!rustEngineAvailable()) {
    throw new RustEngineUnavailableError();
  }
  const runner = tryCreateRustRunner<ChannelValues>(
    assembleParts(
      definition,
      options.approvalEngine !== undefined,
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      options.tools,
      options.subgraphs
    )
  );
  if (runner === null) {
    throw new RustEngineUnavailableError();
  }
  if (options.onEvent !== undefined) {
    runner.subscribe(options.onEvent);
  }
  // ADR 0044: the engine polls this at every node boundary. Captured in a local so the closure
  // reads the caller's signal directly — an abort mid-run is observed at the very next boundary.
  const cancelSignal = options.signal;
  if (cancelSignal !== undefined) {
    runner.cancelWhen(() => cancelSignal.aborted);
  }
  const runId = options.runId ?? generateRunId();
  const state = (await runner.run(
    runId,
    options.initialData ?? {},
    {},
    options.streamTokens ?? false
  )) as unknown as GraphState;
  const governed = await fileApprovalRequests(
    definition,
    state,
    runId,
    options.approvalEngine,
    options.subgraphs
  );
  return {
    state: governed,
    status: governed.status,
    usedRustEngine: true,
    replayJournal: runner.recordedJournal(),
    entryState: runner.recordedEntryState(),
    pendingApprovals: runner.pendingApprovals()
  };
};

/**
 * Resume a previously-suspended catalog run (e.g. past a human gate) from its
 * serialized {@link GraphState}, on the **Rust engine**. The bridge seeds its
 * checkpointer with this state and resumes from it.
 *
 * Throws {@link RustEngineUnavailableError} when the native addon is absent.
 */
export const resumeCatalogGraph = async (
  definition: GraphDefinition,
  state: GraphState,
  options: Pick<
    RunCatalogGraphOptions,
    | "onEvent"
    | "approvalEngine"
    | "providerKeys"
    | "fsPolicy"
    | "skills"
    | "tools"
    | "subgraphs"
    // ADR 0044: a resumed run is just as cancellable as a fresh one — the run loop it re-enters
    // is the same one, so it polls the same seam at the same node boundaries.
    | "signal"
  > & {
    /**
     * Human-granted tools to unlock on resume, each carrying its `{ name, requestedBy,
     * resolvedBy }` provenance. Passed straight through to the Rust bridge, which
     * re-validates the no-self-approval invariant per tool on `Entry::Resume` and writes
     * only the validated names into `__approvedTools`. The control plane (`apps/api`)
     * is the authority on which tools were approved (drawn from the ApprovalEngine), but
     * the engine re-checks the provenance here — defence in depth on the PRODUCTION
     * resume path. Omitted/empty: an ordinary resume that unlocks no tools.
     */
    approvedTools?: ApprovedToolWire[];
  } = {}
): Promise<CatalogRunOutcome> => {
  if (!rustEngineAvailable()) {
    throw new RustEngineUnavailableError();
  }
  const runner = tryCreateRustRunner<ChannelValues>(
    assembleParts(
      definition,
      options.approvalEngine !== undefined,
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      // A resumed run needs its host tools again — a tool-using agent past the gate would
      // otherwise silently degrade to stubs (ADR 0041 D1).
      options.tools,
      // A resumed run needs its subgraph definitions again — a subgraph node resuming past
      // its own child's suspension would otherwise fail to resolve `subgraphId` (ADR 0042).
      options.subgraphs
    )
  );
  if (runner === null) {
    throw new RustEngineUnavailableError();
  }
  if (options.onEvent !== undefined) {
    runner.subscribe(options.onEvent);
  }
  // ADR 0044 — same cooperative cancellation seam as `runCatalogGraph`.
  const cancelSignal = options.signal;
  if (cancelSignal !== undefined) {
    runner.cancelWhen(() => cancelSignal.aborted);
  }
  const resumed = (await runner.resume(
    state,
    options.approvedTools ?? []
  )) as unknown as GraphState;
  // A resume can itself hit a NEW approval gate; file requests for that suspension too.
  const governed = await fileApprovalRequests(
    definition,
    resumed,
    String(resumed.runId) as RunId,
    options.approvalEngine,
    options.subgraphs
  );
  return {
    state: governed,
    status: governed.status,
    usedRustEngine: true,
    replayJournal: runner.recordedJournal()
  };
};

/**
 * Replay-as-evidence (ADR 0038): re-execute a recorded catalog run from `checkpointId`, re-feeding
 * its `replayJournal` (LLM outputs + timestamps from a record-mode {@link runCatalogGraph}) on the
 * Rust engine so the re-derivation is deterministic. A forked, READ-ONLY run — it never files
 * approval requests or opens gates. Returns the replayed state; the caller compares its governed
 * decisions to the attested chain via {@link verifyReplayDecisions}. Requires a native addon with
 * replay support (`engineReplay`); throws otherwise.
 */
export const replayCatalogGraph = async (
  definition: GraphDefinition,
  state: GraphState,
  checkpointId: string,
  replayJournal: string,
  options: Pick<
    RunCatalogGraphOptions,
    "onEvent" | "providerKeys" | "fsPolicy" | "skills" | "subgraphs"
  > = {}
): Promise<CatalogRunOutcome> => {
  if (!rustEngineAvailable()) {
    throw new RustEngineUnavailableError();
  }
  // No approval engine on replay — it is read-only EVIDENCE and must never open a new gate.
  // No host tools either (ADR 0041): a replay must never RE-EXECUTE a tool — bound names degrade
  // to stubs until E2 re-serves recorded tool results from the journal.
  // Subgraphs (ADR 0042/0043 D3): a replayed subgraph node recurses via `execute_subgraph` exactly
  // like a normal run, re-feeding each child's own LLM calls from the SAME journal. This depends on
  // the journal being able to tell a parent's calls apart from a concurrent child's — which ADR 0043
  // D1 (LlmRequest.run_id, tagged with the deterministic `{run_id}:{node_id}[:{index}]` convention)
  // + D2 (ReplayGateway's existing request-equality match now discriminates by run_id for free) +
  // the fork-invariant fix (a replay's forked run_id is normalized back to the record run_id before
  // tagging) now provide. Previously this option was dropped here (`undefined`) specifically because
  // that proof didn't exist yet; a `subgraphId` node with no `subgraphs` supplied still fails loudly
  // (`SubgraphNotFound`) rather than silently diverging.
  const runner = tryCreateRustRunner<ChannelValues>(
    assembleParts(
      definition,
      false,
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      undefined,
      options.subgraphs
    )
  );
  if (runner === null) {
    throw new RustEngineUnavailableError();
  }
  if (options.onEvent !== undefined) {
    runner.subscribe(options.onEvent);
  }
  const replayed = (await runner.replay(
    state,
    checkpointId,
    replayJournal
  )) as unknown as GraphState;
  // The replay re-suspends at the first gate (no approvals seeded): `pendingApprovals` are the
  // subjects the deterministic re-execution requested — what verify-replay compares to the chain.
  return {
    state: replayed,
    status: replayed.status,
    usedRustEngine: true,
    pendingApprovals: runner.pendingApprovals()
  };
};

/** One approval request the seam files, normalized to the `{ description }` subject. */
type SurfacedApprovalRequest = { subject: { description: string } };

/**
 * Normalize one surfaced `approvalRequests` entry's subject to `{ description }`. The
 * Rust agent emits a FLAT string subject (`"tool:<name>"`, see agents-core
 * `ApprovalRequestItem`); the TS handler emits a `{ description: "tool:<name>" }`
 * object. Accept both, returning `undefined` for anything else.
 */
const normalizeSubject = (request: unknown): SurfacedApprovalRequest | undefined => {
  if (!isRecord(request)) {
    return undefined;
  }
  const subject = (request as { subject?: unknown }).subject;
  if (typeof subject === "string") {
    return { subject: { description: subject } };
  }
  if (isRecord(subject) && typeof (subject as { description?: unknown }).description === "string") {
    return { subject: { description: (subject as { description: string }).description } };
  }
  return undefined;
};

/** Read + normalize an agent output channel's `approvalRequests` off a channel bag. */
const readApprovalRequests = (
  channels: Record<string, unknown>,
  outputChannel: string
): SurfacedApprovalRequest[] => {
  const channel = channels[outputChannel];
  if (channel === null || typeof channel !== "object") {
    return [];
  }
  const requests = (channel as { approvalRequests?: unknown }).approvalRequests;
  if (!Array.isArray(requests)) {
    return [];
  }
  return requests
    .map(normalizeSubject)
    .filter((request): request is SurfacedApprovalRequest => request !== undefined);
};

/**
 * Read a subgraph node's child run id off `__subgraphRuns[nodeId]` (recorded by the
 * engine once the child has actually started), falling back to the same deterministic
 * `<runId>:<nodeId>` the Rust bridge computes (`subgraph_run_id`, `runtime.rs`) for a
 * child that hasn't recorded one yet (e.g. still on its first, not-yet-suspended pass).
 */
const readSubgraphRunId = (
  channels: Record<string, unknown>,
  runId: string,
  nodeId: string
): string => {
  const runs = channels[SUBGRAPH_RUNS_CHANNEL];
  if (isRecord(runs)) {
    const existing = runs[nodeId];
    if (typeof existing === "string") {
      return existing;
    }
  }
  return `${runId}:${nodeId}`;
};

/** Read a child run's round-trip snapshot off `__subgraphStates[childRunId]`, if present. */
const readSubgraphChildState = (
  channels: Record<string, unknown>,
  childRunId: string
): { channels: Record<string, unknown>; status: unknown; currentNodeId: unknown } | undefined => {
  const states = channels[SUBGRAPH_STATES_CHANNEL];
  if (!isRecord(states)) {
    return undefined;
  }
  const child = states[childRunId];
  if (!isRecord(child) || !isRecord(child.channels)) {
    return undefined;
  }
  return { channels: child.channels, status: child.status, currentNodeId: child.currentNodeId };
};

/**
 * Subject prefix for a `human-gate` node's own {@link ApprovalEngine} request (issue
 * #496), distinct from {@link TOOL_SUBJECT_PREFIX}-style tool subjects an agent files —
 * the control plane uses this to tell a rejected GATE apart from a rejected TOOL when
 * deciding whether a run becomes `"rejected"` (a tool rejection just leaves a tool
 * unlocked; a gate rejection must block `resume()` outright).
 */
export const GATE_SUBJECT_PREFIX = "gate:";

/**
 * File one {@link ApprovalEngine} request for a suspended `human-gate` node, if the
 * node at `currentNodeId` is one — a structural gate has no `approvalRequests` payload
 * of its own (unlike an agent's gated tool call), so this reads the node type directly
 * rather than a channel. Returns 0 or 1 created ids (a run/child suspends at exactly one
 * node at a time).
 */
const fileGateRequestIfSuspended = async (
  nodes: GraphDefinition["nodes"],
  currentNodeId: unknown,
  runId: RunId,
  idPrefix: string,
  engine: ApprovalEngine
): Promise<string[]> => {
  const node = nodes.find(
    (candidate) => String(candidate.id) === String(currentNodeId) && candidate.type === "human-gate"
  );
  if (node === undefined) {
    return [];
  }
  const created = await engine.request({
    runId,
    nodeId: `${idPrefix}${String(node.id)}` as NodeId,
    requestedBy: `${idPrefix}${String(node.id)}`,
    subject: { description: `${GATE_SUBJECT_PREFIX}${idPrefix}${String(node.id)}` }
  });
  return [String(created.id)];
};

/**
 * File one {@link ApprovalEngine} request per gated tool surfaced by ONE graph's own
 * agent nodes (the top-level run, or — recursively — one direct child's own nodes),
 * reading from `channels` (the run's own `state.channels`, or a child's nested
 * `__subgraphStates[childRunId].channels`). `runId` is the run this request is genuinely
 * FILED under — the top-level run's own id for a top-level node, or the CHILD's own
 * deterministic run id for a child's node (never the parent's — a request filed under
 * the wrong runId is not attributable to the run that actually raised it: a later
 * `ApprovalEngine.getPending(childRunId)`/`loadAttestationChain(childRunId)` must find
 * it). `idPrefix` (empty for the top level; `"<childRunId>:"` for a child) only
 * qualifies `nodeId`/`requestedBy` for human-readability — it is not what makes the
 * request child-attributable; `runId` is.
 */
const fileForGraphNodes = async (
  nodes: GraphDefinition["nodes"],
  channels: Record<string, unknown>,
  runId: RunId,
  idPrefix: string,
  engine: ApprovalEngine
): Promise<string[]> => {
  const ids: string[] = [];
  for (const node of nodes) {
    const agent = readAgentCarrier(node.metadata);
    if (agent === undefined) {
      continue;
    }
    const outputChannel = agent.outputChannel ?? DEFAULT_AGENT_OUTPUT_CHANNEL;
    for (const request of readApprovalRequests(channels, outputChannel)) {
      const created = await engine.request({
        runId,
        nodeId: `${idPrefix}${String(node.id)}` as NodeId,
        requestedBy: `${idPrefix}${String(node.id)}`,
        subject: request.subject
      });
      ids.push(String(created.id));
    }
  }
  return ids;
};

/**
 * File one {@link ApprovalEngine} request per gated tool surfaced by a suspended
 * catalog run, and stash the returned ids in the `__approvalIds` channel of the
 * returned state — mirroring the TS `createAgentNodeHandler` emission pattern
 * (`requestedBy = nodeId`, the agent's own subject). The agent is the requester; a
 * human (a different principal) resolves it out of band, which the engine enforces.
 *
 * ADR 0042 (product ADR 0068 D5.4, ailu-engine#177): also recurses into a DIRECT
 * child's own nodes when that child itself suspended for approval — `execute_subgraph`
 * propagates the child's suspension to the parent, but the child's own
 * `approvalRequests` live in its nested `__subgraphStates[childRunId].channels`
 * snapshot, invisible to the top-level walk alone. Scoped identically to D5.3's own
 * run-gate injection: a single, non-fan-out `subgraphId` reference only (a
 * deterministic `<runId>:<nodeId>` child id exists for that case); a nested subgraph
 * inside that child, or `mapSubgraph`'s dynamic N-child fan-out, is NOT walked here —
 * same "no precomputable id at this point" reasoning D5 already established, left for a
 * follow-up rather than expanding this fix's scope.
 *
 * ADR 0068 issue #496: also files ONE request when the suspended node itself is a
 * `human-gate` (top-level or a direct child's own) — `execute_node` suspends a
 * `human-gate` unconditionally, with NO `ApprovalEngine` involvement of its own kind
 * (unlike an agent's `suspendForApproval`, it carries no `approvalRequests` payload).
 * Without this, `ensureNoPendingApprovals` (the control plane's ONLY resume gate) sees
 * nothing pending and a `human-gate` — including D5.3's own injected `__run_gate` node —
 * delays a resume but never actually authorizes one.
 *
 * No-ops (returns the state unchanged) when no engine is given or the run is not
 * suspended. Idempotency: a run that already carries stashed ids (a state that was
 * governed once) is skipped entirely, so re-driving a suspended state does not
 * double-file — for the parent's own gate, a child's, or a human-gate node.
 */
const fileApprovalRequests = async (
  definition: GraphDefinition,
  state: GraphState,
  runId: RunId,
  engine: ApprovalEngine | undefined,
  subgraphs: GraphDefinition[] | undefined
): Promise<GraphState> => {
  if (engine === undefined || state.status !== "suspended") {
    return state;
  }
  const channels = { ...(state.channels as Record<string, unknown>) };
  const alreadyStashed = Array.isArray(channels[APPROVAL_IDS_CHANNEL])
    ? (channels[APPROVAL_IDS_CHANNEL] as unknown[]).length > 0
    : false;
  if (alreadyStashed) {
    return state;
  }

  const ids = await fileForGraphNodes(definition.nodes, channels, runId, "", engine);
  ids.push(
    ...(await fileGateRequestIfSuspended(definition.nodes, state.currentNodeId, runId, "", engine))
  );

  const subgraphsById = new Map((subgraphs ?? []).map((subgraph) => [subgraph.id, subgraph]));
  for (const node of definition.nodes) {
    if (node.type !== "subgraph" || node.subgraphId === undefined || node.mapSubgraph !== undefined) {
      // Not a direct single-child subgraph reference — mapSubgraph fan-out and
      // anything without a resolvable subgraphId are out of scope here (see doc above).
      continue;
    }
    const child = subgraphsById.get(node.subgraphId);
    if (child === undefined) {
      continue;
    }
    const childRunId = readSubgraphRunId(channels, String(runId), String(node.id));
    const childState = readSubgraphChildState(channels, childRunId);
    if (childState === undefined || childState.status !== "suspended") {
      continue;
    }
    const idPrefix = `${childRunId}:`;
    ids.push(
      ...(await fileForGraphNodes(
        child.nodes,
        childState.channels,
        childRunId as RunId,
        idPrefix,
        engine
      )),
      ...(await fileGateRequestIfSuspended(
        child.nodes,
        childState.currentNodeId,
        childRunId as RunId,
        idPrefix,
        engine
      ))
    );
  }

  if (ids.length === 0) {
    return state;
  }
  return { ...state, channels: { ...channels, [APPROVAL_IDS_CHANNEL]: ids } };
};

/** Type guard a node carries either catalog carrier. Useful to decide the run path. */
export const isCatalogGraph = (definition: GraphDefinition): boolean =>
  definition.nodes.some(
    (node) =>
      readComponentCarrier(node.metadata) !== undefined ||
      readAgentCarrier(node.metadata) !== undefined
  );
