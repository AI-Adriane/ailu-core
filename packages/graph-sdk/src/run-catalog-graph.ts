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
 *   - a MAPAGENTS node carries `node.metadata.mapAgents = { overChannel, joinAt, subAgent }`
 *
 * This is the seam the control plane (`apps/api`) uses to EXECUTE a graph built from
 * the catalog. The engine reads each node's metadata and builds its own spec
 * (`spec_from_catalog`, ADR 0045 D3.2 — the same for every SDK); this seam hands it the
 * definition and the names of the caller's bindings, then drives the run on the
 * **Rust engine** via `@ailu-ai/napi`.
 *
 * Unlike {@link import("./builder.js").GraphBuilder}, there are no TS handler closures
 * here — components and agents run **natively** in Rust, and a plain action/tool node
 * runs the caller's host node binding, or is an inert step (an empty channel update).
 * The carrier IS the wiring.
 *
 * The engine also decides which approvals a governed run files and whether a resume may
 * go on (ADR 0045 D3.1); the `approvalEngine` only stores the requests.
 *
 * The carrier readers below mirror the canonical Zod schema in
 * `@ailu-ai/contracts` (`node-metadata.ts`) and the engine's own; they serve
 * {@link isCatalogGraph}. The control plane is free to validate the carrier with the
 * contracts schema before handing the definition to this runner.
 */

import type { GraphDefinition, GraphState, NodeId, RunId } from "@ailu-ai/graph-core";
import type { RunEvent } from "@ailu-ai/graph-runtime";
import type { ModelTier } from "@ailu-ai/llm-gateway";
// Type-only: keeps the ApprovalEngine contract without pulling its Pg/db implementation
// (and a `pg` dependency) into consumers such as the Studio bundle.
import type { ApprovalEngine, ApprovalId } from "@ailu-ai/approval-engine";

import type {
  EfficiencyMiddlewareSpec,
  FsPolicyRule,
  RustToolBinding,
  RustToolSpec,
  SkillRecord
} from "./agent-node.js";
import { APPROVAL_IDS_CHANNEL } from "./agent-node.js";

import {
  engineApprovalPlan,
  engineApprovalsToCheck,
  engineCatalogSpec,
  engineResumeProblems,
  rustEngineAvailable,
  tryCreateRustRunner,
  type ApprovedToolWire,
  type AsyncNodeFn,
  type RustRunnerParts
} from "./rust-engine.js";
import type { ChannelValues } from "./typed.js";
import { ApprovalNotGrantedError, HostNodeBindingError } from "./errors.js";

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
  /** ailu-core#284 — web search by the model provider (→ Rust `webSearch`). */
  webSearch?: { maxUses: number; allowedDomains?: string[]; blockedDomains?: string[] };
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

/** What a host node receives for one execution (ADR 0045 D2.3). */
export type HostNodeInput = {
  /** The node's id. */
  nodeId: string;
  /** The run's channels when the node starts. */
  channels: Record<string, unknown>;
  /**
   * sha256 of (run id, node id, state version at the node's entry). A retry of the same step from
   * the same checkpoint — the caller's worker re-running a run, a node's retry policy — gets the
   * same key; the next execution of the node in a loop gets another. Key an external effect (a
   * message sent, a record written) on it to perform it at most once.
   */
  effectKey: string;
};

/**
 * A host node (ADR 0045 D2.3): a plain action or tool node of a catalog graph whose step is the
 * caller's code. `execute` returns the channel update. The engine journals each execution in record
 * mode, and a replay serves the journal — it never calls `execute` again.
 */
export type HostNodeBinding = {
  /** The node's id, in the graph or one of its subgraphs. */
  id: string;
  execute: (input: HostNodeInput) => Promise<Record<string, unknown>> | Record<string, unknown>;
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
   * state — so a human resolves them out of band (the engine forbids self-approval).
   * Pass the same engine to {@link resumeCatalogGraph}: it refuses to resume until the
   * engine has approved what the run waits on. Absent: the run is ungoverned.
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
   * Host nodes for this run (ADR 0045 D2.3): plain action or tool nodes whose step is your code,
   * bound by node id — a step that writes to the outside world (send a message, file a record)
   * after a human gate, for instance. Each call receives the node's channels and an `effectKey` to
   * perform its effect at most once. Unbound plain nodes keep their empty update. Supplied per CALL,
   * never persisted with the graph; pass them again to {@link resumeCatalogGraph}. A replay never
   * calls them: {@link replayCatalogGraph} serves what the record-mode run journaled.
   *
   * Throws {@link HostNodeBindingError} when a binding names no plain node of the graph or its
   * subgraphs, or names one twice.
   */
  nodes?: HostNodeBinding[];
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

const generateRunId = (): RunId => {
  const random = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
  return `run_${random}` as RunId;
};

/**
 * Adapt a {@link HostNodeBinding} to the runner's node seam. It refuses to run without an effect
 * key rather than hand the binding an empty one, on which every effect would collide.
 */
const hostNodeFn =
  (binding: HostNodeBinding): AsyncNodeFn<ChannelValues> =>
  async (state, context) => {
    if (context.effectKey === undefined || context.effectKey.length === 0) {
      throw new HostNodeBindingError(
        binding.id,
        "the native engine sent no effect key (@ailu-ai/napi is older than 2.2.0)"
      );
    }
    return binding.execute({
      nodeId: binding.id,
      channels: { ...state.channels },
      effectKey: context.effectKey
    });
  };

/**
 * Assemble the {@link RustRunnerParts} for a catalog graph. The engine reads the graph's
 * carriers and builds its spec ({@link engineCatalogSpec}, ADR 0045 D3.2): component and
 * agent nodes run natively; a plain action/tool node runs its {@link HostNodeBinding} when
 * one is given, and is otherwise an inert node (an empty channel update), so a graph that
 * mixes catalog nodes with plain action/tool nodes still runs end-to-end. The SDK keeps only
 * the functions: host nodes and host tools.
 *
 * Throws {@link HostNodeBindingError} for a binding the engine refuses, and a `TypeError` for
 * a carrier it cannot build a spec from (a `toolSpecs` that is not a list) or a definition
 * without nodes — what the SDK threw there before the engine read the carriers.
 */
const assembleParts = (
  definition: GraphDefinition,
  providerKeys: Record<string, string> | undefined,
  fsPolicy: FsPolicyRule[] | undefined,
  skills: SkillRecord[] | undefined,
  tools: RustToolBinding[] | undefined,
  subgraphs: GraphDefinition[] | undefined,
  nodes: HostNodeBinding[] | undefined
): RustRunnerParts<ChannelValues> => {
  const built = engineCatalogSpec({
    graph: definition,
    subgraphs: subgraphs ?? [],
    hostNodes: (nodes ?? []).map((binding) => binding.id),
    hostTools: (tools ?? []).map((binding) => binding.name),
    providerKeys,
    fsPolicy,
    skills
  });
  if ("error" in built) {
    const { kind, message, nodeId, reason } = built.error;
    if (kind === "hostNodeBinding") {
      throw new HostNodeBindingError(nodeId ?? "", reason ?? message);
    }
    throw new TypeError(message);
  }
  // A node that carries a mapAgents key that does not read would never fan out: no silent caps.
  for (const warning of built.warnings) {
    console.warn(`[ailu] ${warning}`);
  }
  const spec = built.spec as { hostNodeIds?: string[]; jsToolNames?: string[] };

  return {
    definition,
    // ADR 0042 (product ADR 0068 — child workflows): child graphs for `subgraph`-type nodes,
    // supplied per CALL like `tools`/`skills`. The engine flattened their nodes into the spec.
    subgraphs: subgraphs ?? [],
    // ADR 0045 D2.3 — the bound plain nodes; an unbound one gets an empty update from the runner.
    nodeFns: new Map((nodes ?? []).map((binding) => [binding.id, hostNodeFn(binding)])),
    // ADR 0041 D1 — per-run host tools, dispatched through the napi `on_node` `kind:"tool"` seam
    // for the names the spec lists in `jsToolNames`. No bindings → every agent toolName is a
    // native or no-op stub, as before.
    toolFns: new Map((tools ?? []).map((binding) => [binding.name, binding.execute])),
    conditions: new Map(),
    agents: new Map(),
    components: new Map(),
    mapAgents: new Map(),
    jsNodeIds: new Set(spec.hostNodeIds ?? []),
    jsToolNames: new Set(spec.jsToolNames ?? []),
    providerKeys,
    fsPolicy,
    skills,
    catalogSpec: built.spec
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
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      options.tools,
      options.subgraphs,
      options.nodes
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
    undefined,
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
 * With an `approvalEngine`, the resume first checks the engine: every request the run
 * waits on must be decided, no human gate may be rejected, and every granted tool must
 * match an approved request. Otherwise it throws {@link ApprovalNotGrantedError} and
 * nothing runs.
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
    | "nodes"
    | "subgraphs"
    // ADR 0044: a resumed run is just as cancellable as a fresh one — the run loop it re-enters
    // is the same one, so it polls the same seam at the same node boundaries.
    | "signal"
  > & {
    /**
     * Human-granted tools to unlock on resume, each carrying its `{ name, requestedBy,
     * resolvedBy }` provenance. Passed straight through to the Rust bridge, which
     * re-validates the no-self-approval invariant per tool on `Entry::Resume` and writes
     * only the validated names into `__approvedTools`. With an `approvalEngine`, each
     * grant must also match a request the engine records as approved by `resolvedBy`.
     * Omitted/empty: an ordinary resume that unlocks no tools.
     */
    approvedTools?: ApprovedToolWire[];
  } = {}
): Promise<CatalogRunOutcome> => {
  if (!rustEngineAvailable()) {
    throw new RustEngineUnavailableError();
  }
  if (options.approvalEngine !== undefined) {
    await ensureApprovalsGranted(
      definition,
      state,
      options.approvalEngine,
      options.approvedTools ?? [],
      options.subgraphs
    );
  }
  const runner = tryCreateRustRunner<ChannelValues>(
    assembleParts(
      definition,
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      // A resumed run needs its host tools again — a tool-using agent past the gate would
      // otherwise silently degrade to stubs (ADR 0041 D1).
      options.tools,
      // A resumed run needs its subgraph definitions again — a subgraph node resuming past
      // its own child's suspension would otherwise fail to resolve `subgraphId` (ADR 0042).
      options.subgraphs,
      // And its host nodes: the step past a gate is typically the one that acts (ADR 0045).
      options.nodes
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
  // A resume can itself hit a NEW approval gate; file requests for that suspension too. The ids
  // stashed for the previous suspension ride along in the state: the engine drops them when the
  // run now waits on something else — otherwise the new gate would never be filed.
  const governed = await fileApprovalRequests(
    definition,
    resumed,
    state,
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
      options.providerKeys,
      options.fsPolicy,
      options.skills,
      undefined,
      options.subgraphs,
      // No host nodes either (ADR 0045 D1.4): the engine serves each from the journal, never
      // calling one — and a journal recorded before 2.2.0 replays them as empty steps, as before.
      undefined
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

/**
 * Subject prefix for a `human-gate` node's own {@link ApprovalEngine} request (issue
 * #496), distinct from the `tool:<name>` subjects an agent files — the control plane uses
 * this to tell a rejected GATE apart from a rejected TOOL when deciding whether a run
 * becomes `"rejected"` (a tool rejection just leaves a tool unlocked; a gate rejection
 * must block `resume()` outright).
 */
export const GATE_SUBJECT_PREFIX = "gate:";

const withoutApprovalIds = (state: GraphState): GraphState => ({
  ...state,
  channels: { ...(state.channels as Record<string, unknown>), [APPROVAL_IDS_CHANNEL]: [] }
});

/**
 * File the approval requests a suspended catalog run waits on, and keep their ids in its
 * `__approvalIds` channel. The engine decides what is filed (`filing_plan`, ADR 0045 D3.1): one
 * request per gated tool an agent asked for (`requestedBy` = its node id, the agent's own
 * subject), one for the human gate the run stopped at (`gate:<node id>`), and the same for a
 * direct child run suspended inside a subgraph node — filed under the child's run id, its node
 * ids prefixed with it. Nothing when the run is not suspended or already stashed its ids, so
 * re-driving a governed state does not file twice. The agent is the requester; a human (another
 * principal) resolves the request out of band, which the engine enforces.
 *
 * After a resume (`previousState` given), a run that now waits on something else first drops the
 * ids stashed for its previous wait — with or without an approval engine, as before.
 */
const fileApprovalRequests = async (
  definition: GraphDefinition,
  state: GraphState,
  previousState: GraphState | undefined,
  engine: ApprovalEngine | undefined,
  subgraphs: GraphDefinition[] | undefined
): Promise<GraphState> => {
  const plan = engineApprovalPlan({
    graph: definition,
    subgraphs: subgraphs ?? [],
    state,
    previousState
  });
  const kept = plan.clearApprovalIds ? withoutApprovalIds(state) : state;
  if (engine === undefined || plan.requests.length === 0) {
    return kept;
  }
  const ids: string[] = [];
  for (const request of plan.requests) {
    const created = await engine.request({
      runId: request.runId as RunId,
      nodeId: request.nodeId as NodeId,
      requestedBy: request.requestedBy,
      subject: request.subject
    });
    ids.push(String(created.id));
  }
  return {
    ...kept,
    channels: { ...(kept.channels as Record<string, unknown>), [APPROVAL_IDS_CHANNEL]: ids }
  };
};

/**
 * Refuse a governed resume the engine has not authorized (`resume_problems`, ADR 0045 D3.1): a
 * request the run waits on is still pending or unknown, a human gate was rejected, a request was
 * approved by its own requester, a granted tool has no matching approved request, or the state
 * comes from a run started without the engine (its approvals were never recorded). The approval
 * engine is only read: the records of the ids the run stashed.
 */
const ensureApprovalsGranted = async (
  definition: GraphDefinition,
  state: GraphState,
  engine: ApprovalEngine,
  approvedTools: ApprovedToolWire[],
  subgraphs: GraphDefinition[] | undefined
): Promise<void> => {
  const approvals: Record<string, unknown> = {};
  for (const id of engineApprovalsToCheck(state)) {
    const request = await engine.getById(id as ApprovalId);
    approvals[id] =
      request === undefined
        ? null
        : {
            status: request.status,
            subject: request.subject,
            requestedBy: request.requestedBy,
            resolvedBy: request.resolvedBy
          };
  }
  const problems = engineResumeProblems({
    graph: definition,
    subgraphs: subgraphs ?? [],
    state,
    approvedTools,
    approvals
  });
  if (problems.length > 0) {
    throw new ApprovalNotGrantedError(String(state.runId), problems);
  }
};

/** Type guard a node carries either catalog carrier. Useful to decide the run path. */
export const isCatalogGraph = (definition: GraphDefinition): boolean =>
  definition.nodes.some(
    (node) =>
      readComponentCarrier(node.metadata) !== undefined ||
      readAgentCarrier(node.metadata) !== undefined
  );
