import { createRequire } from "node:module";

import type { GraphDefinition, GraphState, NodeId, RunId } from "@adriane-ai/graph-core";
import type { RunEvent } from "@adriane-ai/graph-runtime";

import type { ModelTier } from "@adriane-ai/llm-gateway";

import type {
  EfficiencyMiddlewareSpec,
  FsPolicyRule,
  MemoryConfig,
  RustAgentConfig,
  RustMapAgentConfig,
  SkillConfig,
  SkillRecord
} from "./agent-node.js";
import type { RustComponentConfig } from "./components.js";
import type { ChannelValues, TypedGraphState } from "./typed.js";

/**
 * Optional bridge to the Rust engine's async run/resume/approve entry points
 * (`@adriane-ai/napi`). Mirrors {@link import("./rust-validator.js").tryRustValidate}:
 * when the native addon is present, graph **execution** can run on the Rust engine
 * (via `engine_run`/`engine_resume`/`engine_approve_and_resume`), with the SDK's TS
 * condition predicates and node/tool seams called back from Rust over a
 * ThreadsafeFunction. The Rust engine is the required production runtime; when the
 * addon is absent (development, tests, or an uncovered platform) this loader returns
 * `null` and {@link import("./compiled-graph.js").CompiledGraph} uses the in-process
 * TypeScript runtime instead.
 *
 * Boundary contract (napi 2.16, Phase F): the Rust seam now **awaits** the JS
 * callback's returned `Promise` — `call_async::<Promise<T>>(..).await?.await?` drives
 * a returned thenable to its resolved value. So `on_node` returns a `Promise<string>`
 * (channel-update / tool-result JSON) and `on_condition` returns a `Promise<string>`
 * (a boolean-ish string the Rust side reads with `parse_bool`). This is what lets the
 * SDK's genuinely-async TS node handlers and tool `execute` fns round-trip through
 * Rust without aborting the process. `on_event` stays fire-and-forget.
 */
export type EngineNodeCallback = (payloadJson: string) => Promise<string>;
export type EngineConditionCallback = (payloadJson: string) => Promise<string>;
export type EngineEventCallback = (payloadJson: string) => void;
/**
 * Cooperative cancellation (ADR 0044). Polled by the Rust run loop at every node BOUNDARY;
 * resolve `"true"` to stop the run cleanly with status `cancelled` after its last checkpoint.
 * Deliberately shaped like `on_condition` (awaited, value-returning) rather than `on_event`
 * (fire-and-forget) — only a seam that can ANSWER can stop a run.
 */
export type EngineCancelCallback = () => Promise<string>;

type NativeEngine = {
  engineRun(
    specJson: string,
    onNode: EngineNodeCallback,
    onCondition: EngineConditionCallback,
    onEvent: EngineEventCallback,
    isCancelled?: EngineCancelCallback
  ): Promise<string>;
  engineResume(
    specJson: string,
    onNode: EngineNodeCallback,
    onCondition: EngineConditionCallback,
    onEvent: EngineEventCallback,
    isCancelled?: EngineCancelCallback
  ): Promise<string>;
  engineApproveAndResume(
    specJson: string,
    onNode: EngineNodeCallback,
    onCondition: EngineConditionCallback,
    onEvent: EngineEventCallback,
    isCancelled?: EngineCancelCallback
  ): Promise<string>;
  engineSignal(
    specJson: string,
    signalName: string,
    payloadJson: string,
    onNode: EngineNodeCallback,
    onCondition: EngineConditionCallback,
    onEvent: EngineEventCallback,
    isCancelled?: EngineCancelCallback
  ): Promise<string>;
  /**
   * Replay-as-evidence (ADR 0038). OPTIONAL + feature-detected: an older prebuilt addon may lack
   * it, so it is deliberately NOT part of the {@link hasEngineFns} hard guard — its absence must
   * never drop ALL Rust execution to the TS fallback (it only disables replay).
   */
  engineReplay?(
    specJson: string,
    checkpointId: string,
    onNode: EngineNodeCallback,
    onCondition: EngineConditionCallback,
    onEvent: EngineEventCallback
  ): Promise<string>;
};

let cachedNative: NativeEngine | null | undefined;

const hasEngineFns = (mod: unknown): mod is NativeEngine =>
  typeof mod === "object" &&
  mod !== null &&
  typeof (mod as NativeEngine).engineRun === "function" &&
  typeof (mod as NativeEngine).engineResume === "function" &&
  typeof (mod as NativeEngine).engineApproveAndResume === "function" &&
  typeof (mod as NativeEngine).engineSignal === "function";

const loadNativeEngine = (): NativeEngine | null => {
  if (cachedNative !== undefined) {
    return cachedNative;
  }
  try {
    const requireFn = createRequire(import.meta.url);
    const mod: unknown = requireFn("@adriane-ai/napi");
    cachedNative = hasEngineFns(mod) ? mod : null;
  } catch {
    cachedNative = null;
  }
  return cachedNative;
};

/** True when the native addon exposes the async run bridge (execution can use Rust). */
export const rustEngineAvailable = (): boolean => loadNativeEngine() !== null;

/**
 * An async node-update producer for the Rust seam: given the (channels-only) typed
 * state, resolve to the channel update. The SDK adapts its async `NodeHandler` into
 * this directly — the Rust seam awaits the returned promise (Phase F), so a handler
 * that does real async work (I/O, an LLM call) round-trips faithfully.
 */
export type AsyncNodeFn<TState extends ChannelValues> = (
  state: TypedGraphState<TState>
) => Promise<Record<string, unknown>>;

/** An async tool `execute` for the Rust seam: input in, tool-result value out. */
export type AsyncToolFn = (input: unknown) => Promise<unknown>;

/**
 * What the SDK passes to {@link RustGraphRunner}: the things the Rust engine cannot
 * reconstruct itself — async node-update producers (for JS node ids), async tool
 * executes (for JS tool names), and condition predicates — plus the serializable
 * agent configs and the set of JS node ids.
 */
export type RustRunnerParts<TState extends ChannelValues> = {
  definition: GraphDefinition;
  /**
   * Child graphs for `subgraph`-type nodes (Rust `EngineSpec.subgraphs`). Their node
   * handlers / conditions are flattened into {@link RustRunnerParts.nodeFns} /
   * {@link RustRunnerParts.conditions} (by global node id); these definitions tell the
   * Rust engine how to traverse each child. Empty for graphs with no subgraphs.
   */
  subgraphs: GraphDefinition[];
  /** Async node-update producers, keyed by node id (Rust `on_node`, `kind:"node"`). */
  nodeFns: Map<string, AsyncNodeFn<TState>>;
  /** Async tool executes, keyed by tool name (Rust `on_node`, `kind:"tool"`). */
  toolFns: Map<string, AsyncToolFn>;
  /** Named condition predicates, keyed by condition name (Rust `on_condition`). */
  conditions: Map<string, (state: TypedGraphState<TState>) => boolean>;
  /** Per-agent-node serializable config. */
  agents: Map<string, RustAgentConfig>;
  /**
   * Per-component-node serializable config (Phase C `componentNodes` carrier), keyed
   * by node id. The node runs a native Rust component handler; its id may *also* be in
   * {@link jsNodeIds} (its TS fallback handler) — Rust takes the component path.
   */
  components: Map<string, RustComponentConfig>;
  /** Per `mapAgents`-node dynamic-fan-out config (ADR 0027 phase 4b), keyed by node id. */
  mapAgents: Map<string, RustMapAgentConfig>;
  /** Node ids whose handler is a JS closure (action / custom / tool nodes). */
  jsNodeIds: Set<string>;
  /** Tool names that are backed by a JS `execute` (in {@link toolFns}). */
  jsToolNames: Set<string>;
  /**
   * Per-provider API keys injected by the control plane (ADR 0010), keyed by provider
   * slug (`openai`, `anthropic`, `mistral`, …). The Rust gateway resolves each agent's
   * provider key tenant-key-first, then process env. Omitted for runs that rely purely
   * on the host env (e.g. local dev, tests).
   */
  providerKeys?: Record<string, string>;
  /**
   * Per-path filesystem permission rules (ADR 0024 phase 2b), compiled into the run's
   * StaticPathPolicy and applied to every fs-enabled agent. Empty/omitted = fail-closed
   * read-only everywhere.
   */
  fsPolicy?: FsPolicyRule[];
  /**
   * The tenant's governed skills for this run (ADR 0049 B-3) → Rust `EngineSpec.skills`. The engine
   * builds a run-scoped, tenant-isolated skill store from these; each agent's SkillMiddleware selects
   * from it. Omitted/empty → the OSS shared in-memory store (no skills).
   */
  skills?: SkillRecord[];
};

/** The `agents` map serialized for the wire (matches Rust `AgentSpec`, camelCase). */
type AgentSpecWire = {
  provider: string;
  model?: string;
  /** Capability tier; the Rust bridge resolves it via `ModelPolicy` when no `model`. */
  tier?: ModelTier;
  /** Custom OpenAI-compatible endpoint (→ Rust `baseUrl`) and the env var naming its key. */
  baseUrl?: string;
  apiKeyEnv?: string;
  system?: string;
  toolNames: string[];
  maxIterations?: number;
  suspendForApproval: boolean;
  approvalToolNames: string[];
  outputChannel: string;
  /** ADR 0014 token-efficiency knobs (camelCase → Rust AgentSpec `outputStyle`/`contextBudget`). */
  outputStyle?: "terse";
  contextBudget?: number;
  /** ADR 0022/0023 — durable channel the `writeTodos` list is persisted into (→ Rust `todosChannel`). */
  todosChannel?: string;
  /** ADR 0030 phase 9e — channel carrying the run's multimodal input blocks (→ Rust `inputBlocksChannel`). */
  inputBlocksChannel?: string;
  /** ADR 0026 phase 11 — governed long-term memory overlay (→ Rust `memory`). */
  memory?: MemoryConfig;
  /** ADR 0035 phase 12 — governed skills (progressive disclosure) overlay (→ Rust `skills`). */
  skills?: SkillConfig;
  /** ADR 0024 phase 2b — opt this agent into the governed virtual filesystem tools. */
  enableFs?: boolean;
  /** ADR 0025 phase 3d — the SDK-resolved efficiency middleware list (→ Rust `resolvedMiddleware`). */
  resolvedMiddleware?: EfficiencyMiddlewareSpec[];
};

/**
 * One native component node serialized for the wire (matches Rust `ComponentNodeSpec`,
 * camelCase): a component `kind` plus its `params` object. Such a node runs the Rust
 * `adriane_components` handler natively instead of the JS seam.
 */
type ComponentNodeSpecWire = {
  kind: string;
  params: Record<string, unknown>;
};

/** One `mapAgents` dynamic-fan-out node serialized for the wire (matches Rust `MapAgentSpec`). */
type MapAgentSpecWire = {
  overChannel: string;
  joinAt: string;
  agent: AgentSpecWire;
  suspendForApproval: boolean;
};

/**
 * One granted tool on the approve path, with the governance provenance the Rust
 * guard-rail validates (matches Rust `ApprovedTool`, camelCase): the principal who
 * *requested* the approval and the (distinct) principal who *resolved* it. The bridge
 * rejects the resume if `resolvedBy` is empty or equals `requestedBy` (no self-approval).
 */
export type ApprovedToolWire = {
  name: string;
  requestedBy: string;
  resolvedBy: string;
  /**
   * Content-scoped grant key (ADR 0024 phase 2c): `"<name>#<sha256(input)>"` for a
   * guarded fs write, pinning the grant to the exact call. When set, the engine writes
   * THIS key (not the bare name) into `__approvedTools`. Omitted for a name-only grant.
   */
  key?: string;
};

/** The `EngineSpec` shape the Rust bridge deserializes (camelCase). */
type EngineSpecWire = {
  graph: GraphDefinition;
  /** Child graphs for `subgraph`-type nodes; the bridge registers their nodes too. */
  subgraphs: GraphDefinition[];
  /** Dynamic-message inbox to pre-queue (`send`): per node id, FIFO inputs. */
  inbox?: Record<string, unknown[]>;
  runId?: string;
  /**
   * Opt-in per-token streaming (ADR 0033 phase 13). When `true`, agent nodes drive the
   * gateway's streaming path and emit observational `token_delta` events; when absent —
   * the default — agents `complete()` and the run is byte-identical. The SDK sets it only
   * for a `messages`-mode stream, the one consumer that renders token-granular output.
   */
  streamTokens?: boolean;
  initialData?: Record<string, unknown>;
  state?: GraphState;
  /**
   * Replay-as-evidence (ADR 0038): a recorded run journal (`{ decisions, clock }` JSON) re-fed
   * on the replay path so re-execution re-serves the original LLM outputs + timestamps instead
   * of re-sampling. Set only by the replay path.
   */
  replayJournal?: string;
  approvedTools?: ApprovedToolWire[];
  agents: Record<string, AgentSpecWire>;
  /**
   * Per-node native component configuration, keyed by node id. The Phase C carrier:
   * such a node runs a Rust component handler (built at assemble time from `kind` +
   * `params`) and takes precedence over the JS seam even when its id also appears in
   * {@link EngineSpecWire.jsNodeIds}.
   */
  componentNodes: Record<string, ComponentNodeSpecWire>;
  /** Per-node `mapAgents` dynamic-fan-out config (ADR 0027 phase 4b), keyed by node id. */
  mapAgents: Record<string, MapAgentSpecWire>;
  jsNodeIds: string[];
  jsToolNames: string[];
  /**
   * Per-provider API keys (ADR 0010), keyed by provider slug. The Rust bridge resolves
   * each agent's key tenant-key-first then env; an empty map means env-only resolution.
   */
  providerKeys: Record<string, string>;
  /** Per-path filesystem permission rules (ADR 0024 phase 2b); empty = fail-closed read-only. */
  fsPolicy: FsPolicyRule[];
  /** The tenant's governed skills (ADR 0049 B-3) → Rust `EngineSpec.skills`; empty = OSS shared store. */
  skills: SkillRecord[];
};

/** The `RunOutcome` shape the Rust bridge serializes back. */
type RunOutcomeWire = {
  state: GraphState;
  status: string;
  /**
   * Pending approvals when suspended. For a content-scoped guarded fs write (ADR 0024
   * phase 2c) the item also carries `approvalKey` (the composite grant to send back on
   * approve) + `input` (the path/content, so a reviewer sees what is approved).
   */
  pendingApprovals: { subject: string; reason: string; approvalKey?: string; input?: unknown }[];
  /**
   * Replay-as-evidence (ADR 0038): the recorded LLM I/O + clock journal (`{ decisions, clock }`
   * JSON) when the run executed in record mode (`ADRIANE_LLM_RECORD`); `undefined` otherwise. The
   * control plane persists it to re-feed a later replay.
   */
  replayJournal?: string;
  /**
   * Replay-as-evidence (ADR 0040): the run's ENTRY state (initial state, before the entry node
   * ran), surfaced only on a record-mode start; `undefined` otherwise. The control plane persists it
   * as the checkpoint a later verify-replay seeds `replayFrom` from, to re-derive the run from start.
   */
  entryState?: GraphState;
};

/** Payload the Rust `on_node` seam sends for a JS node handler or a JS tool. */
type NodePayload =
  | { kind: "node"; nodeId: string; input: unknown; state: Record<string, unknown> }
  | { kind: "tool"; name: string; input: unknown };

/** Payload the Rust `on_condition` seam sends for a named predicate. */
type ConditionPayload = { name: string; state: Record<string, unknown> };

/**
 * Drives a graph on the Rust engine through the napi bridge. One instance per
 * {@link import("./compiled-graph.js").CompiledGraph}; safe to call repeatedly.
 *
 * The TS state the Rust seam hands back is **channels-only** (`{ kind, state }` where
 * `state` is the channel map). We reconstruct a {@link TypedGraphState} carrying those
 * channels so TS conditions and node fns see the same `state.channels` they would on
 * the TS engine — which is what preserves the SDK's typed-state semantics across the
 * boundary. The `GraphState` fields the seam doesn't carry are filled with inert
 * placeholders (SDK conditions and the SDK's own node logic read `state.channels`).
 */
export class RustGraphRunner<TState extends ChannelValues> {
  private readonly native: NativeEngine;
  private readonly parts: RustRunnerParts<TState>;
  private readonly eventSubscribers = new Set<(event: RunEvent) => void>();
  /** The last run's recorded journal (ADR 0038, record mode); `undefined` outside record mode. */
  private lastReplayJournal: string | undefined;
  /** The last record-mode run's ENTRY state (ADR 0040); `undefined` outside record mode. */
  private lastEntryState: GraphState | undefined;
  /** The last run's pending approvals (the requested subjects when it suspended on a gate). */
  private lastPendingApprovals: RunOutcomeWire["pendingApprovals"] = [];

  /** Construct only after {@link rustEngineAvailable} returned true. */
  public constructor(native: NativeEngine, parts: RustRunnerParts<TState>) {
    this.native = native;
    this.parts = parts;
  }

  /** The recorded LLM I/O + clock journal from the last run, when it ran in record mode
   * (`ADRIANE_LLM_RECORD`) — the control plane persists it to re-feed a replay (ADR 0038). */
  public recordedJournal(): string | undefined {
    return this.lastReplayJournal;
  }

  /** The ENTRY state of the last record-mode run (ADR 0040) — the control plane persists it as the
   * checkpoint a later verify-replay seeds `replay` from. `undefined` outside record mode. */
  public recordedEntryState(): GraphState | undefined {
    return this.lastEntryState;
  }

  /** The pending approvals (requested subjects) from the last run, when it suspended on a gate.
   * On a replay this is what the deterministic re-execution requested — the faithfulness signal. */
  public pendingApprovals(): RunOutcomeWire["pendingApprovals"] {
    return this.lastPendingApprovals;
  }

  /** Whether the installed native addon supports replay (`engineReplay`) — feature-detected. */
  public canReplay(): boolean {
    return typeof this.native.engineReplay === "function";
  }

  /** Subscribe to forwarded run-lifecycle events. Returns an unsubscribe fn. */
  public subscribe(handler: (event: RunEvent) => void): () => void {
    this.eventSubscribers.add(handler);
    return () => {
      this.eventSubscribers.delete(handler);
    };
  }

  /** Lift the channel map the Rust seam sends into a typed (channels-only) state. */
  private liftState(channels: Record<string, unknown>): TypedGraphState<TState> {
    return {
      runId: "" as RunId,
      graphId: this.parts.definition.id,
      currentNodeId: "" as NodeId,
      status: "running",
      channels: channels as TState,
      version: 0,
      createdAt: "",
      updatedAt: ""
    };
  }

  /**
   * The `on_node` seam: dispatch to an async node fn or tool execute and resolve to
   * its JSON. The Rust side awaits this promise (Phase F), so a handler doing real
   * async work round-trips faithfully. A missing fn resolves to `"{}"` (an empty
   * update) rather than rejecting, matching the Rust side's tolerant `parse_update`.
   */
  private readonly onNode: EngineNodeCallback = async (payloadJson) => {
    const payload = JSON.parse(payloadJson) as NodePayload;
    if (payload.kind === "node") {
      const fn = this.parts.nodeFns.get(payload.nodeId);
      if (fn === undefined) {
        return "{}";
      }
      return JSON.stringify(await fn(this.liftState(payload.state)));
    }
    const tool = this.parts.toolFns.get(payload.name);
    if (tool === undefined) {
      return "{}";
    }
    return JSON.stringify(await tool(payload.input));
  };

  /**
   * The `on_condition` seam: evaluate a named TS predicate against the channels and
   * resolve to its boolean-ish string (`"true"`/`"false"`). The predicate is
   * synchronous (the SDK's {@link TypedCondition} contract), but the Rust side awaits
   * a `Promise<string>`, so we return one. A missing predicate resolves to `"false"`.
   */
  private readonly onCondition: EngineConditionCallback = async (payloadJson) => {
    const payload = JSON.parse(payloadJson) as ConditionPayload;
    const predicate = this.parts.conditions.get(payload.name);
    const value = predicate === undefined ? false : predicate(this.liftState(payload.state));
    return value ? "true" : "false";
  };

  /** The `on_event` seam: forward each run-lifecycle event to subscribers. */
  private readonly onEvent: EngineEventCallback = (payloadJson) => {
    const event = JSON.parse(payloadJson) as RunEvent;
    for (const subscriber of this.eventSubscribers) {
      subscriber(event);
    }
  };

  /**
   * Cooperative cancellation (ADR 0044). Installed by {@link cancelWhen}; `undefined` until
   * then, which is what keeps the napi call shape (and therefore engine behaviour) unchanged
   * for every caller that never asks to cancel.
   */
  private cancelCheck?: () => boolean;

  /**
   * Ask the Rust run loop to stop at the next node boundary whenever `check()` returns true.
   * Polled (never pre-emptive): a node already executing always runs to completion and
   * checkpoints, so cancelling can never tear a run's state.
   */
  public cancelWhen(check: () => boolean): void {
    this.cancelCheck = check;
  }

  /** The `is_cancelled` seam: answer the engine's boundary poll. */
  private readonly onCancelled: EngineCancelCallback = async () =>
    this.cancelCheck?.() === true ? "true" : "false";

  /**
   * The napi argument — `undefined` unless a check was installed, so the optional parameter is
   * simply absent on the call and an older addon sees the exact 4-argument shape it expects.
   */
  private get cancelArg(): EngineCancelCallback | undefined {
    return this.cancelCheck === undefined ? undefined : this.onCancelled;
  }

  private agentWire(config: RustAgentConfig): AgentSpecWire {
    return {
      provider: config.provider,
      model: config.model,
      tier: config.tier,
      baseUrl: config.baseURL,
      apiKeyEnv: config.apiKeyEnv,
      system: config.system,
      toolNames: config.toolNames,
      maxIterations: config.maxIterations,
      suspendForApproval: config.suspendForApproval,
      approvalToolNames: config.approvalToolNames,
      outputChannel: config.outputChannel,
      outputStyle: config.outputStyle,
      contextBudget: config.contextBudget,
      todosChannel: config.todosChannel,
      inputBlocksChannel: config.inputBlocksChannel,
      memory: config.memory,
      skills: config.skills,
      enableFs: config.enableFs,
      resolvedMiddleware: config.resolvedMiddleware
    };
  }

  private buildAgentsWire(): Record<string, AgentSpecWire> {
    const out: Record<string, AgentSpecWire> = {};
    for (const [nodeId, config] of this.parts.agents) {
      out[nodeId] = this.agentWire(config);
    }
    return out;
  }

  private buildMapAgentsWire(): Record<string, MapAgentSpecWire> {
    const out: Record<string, MapAgentSpecWire> = {};
    for (const [nodeId, config] of this.parts.mapAgents) {
      out[nodeId] = {
        overChannel: config.overChannel,
        joinAt: config.joinAt,
        agent: this.agentWire(config.agent),
        suspendForApproval: config.suspendForApproval
      };
    }
    return out;
  }

  private buildComponentsWire(): Record<string, ComponentNodeSpecWire> {
    const out: Record<string, ComponentNodeSpecWire> = {};
    for (const [nodeId, config] of this.parts.components) {
      out[nodeId] = { kind: config.kind, params: config.params };
    }
    return out;
  }

  private outcomeToState(outcomeJson: string): TypedGraphState<TState> {
    const outcome = JSON.parse(outcomeJson) as RunOutcomeWire;
    // ADR 0038: capture a record-mode run's journal so callers can persist it (`recordedJournal`).
    this.lastReplayJournal = outcome.replayJournal;
    // ADR 0040: capture the entry state (record mode) + the run's pending approvals (the requested
    // subjects when it suspended on a gate) — both are inputs/outputs the verify-replay flow reads.
    this.lastEntryState = outcome.entryState;
    this.lastPendingApprovals = outcome.pendingApprovals ?? [];
    // The wire state is already a valid camelCase GraphState; channels are the typed
    // shape declared by the builder, so this cast is exact (no field reshaping).
    return outcome.state as unknown as TypedGraphState<TState>;
  }

  private baseSpec(): Pick<
    EngineSpecWire,
    | "graph"
    | "subgraphs"
    | "agents"
    | "componentNodes"
    | "mapAgents"
    | "jsNodeIds"
    | "jsToolNames"
    | "providerKeys"
    | "fsPolicy"
    | "skills"
  > {
    return {
      graph: this.parts.definition,
      subgraphs: this.parts.subgraphs,
      agents: this.buildAgentsWire(),
      componentNodes: this.buildComponentsWire(),
      mapAgents: this.buildMapAgentsWire(),
      jsNodeIds: [...this.parts.jsNodeIds],
      jsToolNames: [...this.parts.jsToolNames],
      providerKeys: this.parts.providerKeys ?? {},
      fsPolicy: this.parts.fsPolicy ?? [],
      skills: this.parts.skills ?? []
    };
  }

  /** Start a fresh run on the Rust engine. `streamTokens` opts into per-token streaming (ADR 0033). */
  public async run(
    runId: RunId,
    initialData: Record<string, unknown>,
    inbox: Record<string, unknown[]> = {},
    streamTokens = false
  ): Promise<TypedGraphState<TState>> {
    const spec: EngineSpecWire = { ...this.baseSpec(), runId, initialData, inbox, streamTokens };
    const outcomeJson = await this.native.engineRun(
      JSON.stringify(spec),
      this.onNode,
      this.onCondition,
      this.onEvent,
      this.cancelArg
    );
    return this.outcomeToState(outcomeJson);
  }

  /**
   * Resume a suspended run from its serialized state. Optionally carries the
   * human-granted `approvedTools` WITH their `{ name, requestedBy, resolvedBy }`
   * provenance (the production catalog resume path): the Rust bridge re-validates the
   * no-self-approval invariant per tool on `Entry::Resume` and writes only the
   * validated names into `__approvedTools` — so a self-approved (or unresolved) tool
   * aborts the resume here too, not just on the approve path. Omitted (or empty) for
   * an ordinary resume past a non-approval gate, which unlocks no tools.
   */
  public async resume(
    state: GraphState,
    approvedTools: ApprovedToolWire[] = []
  ): Promise<TypedGraphState<TState>> {
    const spec: EngineSpecWire = { ...this.baseSpec(), state, approvedTools };
    const outcomeJson = await this.native.engineResume(
      JSON.stringify(spec),
      this.onNode,
      this.onCondition,
      this.onEvent,
      this.cancelArg
    );
    return this.outcomeToState(outcomeJson);
  }

  /**
   * Grant the approved tools (each carrying its `{ name, requestedBy, resolvedBy }`
   * provenance), then resume. The Rust bridge re-validates the no-self-approval
   * invariant per tool and writes only the validated names into `__approvedTools`
   * before resuming — a self-approved (or unresolved) tool aborts the resume.
   */
  public async approveAndResume(
    state: GraphState,
    approvedTools: ApprovedToolWire[]
  ): Promise<TypedGraphState<TState>> {
    const spec: EngineSpecWire = { ...this.baseSpec(), state, approvedTools };
    const outcomeJson = await this.native.engineApproveAndResume(
      JSON.stringify(spec),
      this.onNode,
      this.onCondition,
      this.onEvent,
      this.cancelArg
    );
    return this.outcomeToState(outcomeJson);
  }

  /**
   * Deliver an external signal to a suspended run, then resume it. The Rust engine
   * injects `payload` into `__signals[name]` and advances past the node that awaited
   * the signal (a `waitForSignal` suspension is one-shot).
   */
  public async signal(
    state: GraphState,
    name: string,
    payload: unknown
  ): Promise<TypedGraphState<TState>> {
    const spec: EngineSpecWire = { ...this.baseSpec(), state };
    const outcomeJson = await this.native.engineSignal(
      JSON.stringify(spec),
      name,
      JSON.stringify(payload ?? null),
      this.onNode,
      this.onCondition,
      this.onEvent,
      this.cancelArg
    );
    return this.outcomeToState(outcomeJson);
  }

  /**
   * Replay-as-evidence (ADR 0038): re-execute the run from `checkpointId`, re-feeding the recorded
   * `replayJournal` (`{ decisions, clock }` JSON from a record-mode run's {@link recordedJournal})
   * so the re-derivation is deterministic — the same LLM outputs + timestamps, not a re-sample. A
   * forked, read-only run that never opens approval gates. Requires the native addon to expose
   * `engineReplay` (a newer prebuilt); guard with {@link canReplay} first.
   */
  public async replay(
    state: GraphState,
    checkpointId: string,
    replayJournal: string
  ): Promise<TypedGraphState<TState>> {
    if (this.native.engineReplay === undefined) {
      throw new Error(
        "the installed @adriane-ai/napi addon has no replay support (engineReplay) — rebuild/upgrade it"
      );
    }
    const spec: EngineSpecWire = { ...this.baseSpec(), state, replayJournal };
    const outcomeJson = await this.native.engineReplay(
      JSON.stringify(spec),
      checkpointId,
      this.onNode,
      this.onCondition,
      this.onEvent
    );
    return this.outcomeToState(outcomeJson);
  }
}

/** Build a {@link RustGraphRunner} if the native engine is present, else `null`. */
export const tryCreateRustRunner = <TState extends ChannelValues>(
  parts: RustRunnerParts<TState>
): RustGraphRunner<TState> | null => {
  const native = loadNativeEngine();
  return native === null ? null : new RustGraphRunner<TState>(native, parts);
};
