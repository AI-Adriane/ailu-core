import type { GraphDefinition, GraphState, NodeId, RunId } from "@ailu-ai/graph-core";
import {
  GraphRuntime,
  InMemoryCheckpointer,
  InMemoryConditionRegistry,
  InMemoryEventBus,
  InMemoryNodeRegistry,
  type Checkpointer,
  type ConditionFn,
  type EventBus,
  type NodeExecutionContext,
  type NodeHandler,
  type RunEvent,
  type StreamEvent,
  type StreamMode
} from "@ailu-ai/graph-runtime";

import { AiluSdkError, ApproverRequiredError, ResumeStateNotFoundError } from "./errors.js";
import { explainRun, type RunExplanation } from "./run-explainer.js";
import {
  type AgentApprovalBinding,
  type FsPolicyRule,
  type RustAgentConfig,
  type RustMapAgentConfig
} from "./agent-node.js";
import type { RustComponentConfig } from "./components.js";
import {
  rustEngineAvailable,
  tryCreateRustRunner,
  type ApprovedToolWire,
  type AsyncNodeFn,
  type AsyncToolFn,
  type RustGraphRunner,
  type RustRunnerParts
} from "./rust-engine.js";
import type { ChannelValues, InitialData, TypedGraphState } from "./typed.js";

/** Options for {@link CompiledGraph.approveAndResume}. */
export type ApproveAndResumeOptions = {
  /**
   * Approval-gated tools the human has granted; they execute on resume. A bare string
   * grants a tool by name; `{ name, key }` grants a CONTENT-SCOPED guarded fs write
   * (ADR 0024 phase 2c) pinned to the exact call — pass the `approvalKey` surfaced on the
   * suspended run's pending approval, so only the approved path+content unlocks.
   */
  approvedTools: Array<string | { name: string; key?: string }>;
  /**
   * Who approved: a person, never the agent that requested the tool. Required. It is
   * recorded as each granted tool's `resolvedBy` and carried to the Rust engine, which
   * rejects the resume if it equals the tool's requester (no self-approval).
   */
  resolvedBy: string;
};

const generateRunId = (): RunId => {
  const random = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
  return `run_${random}` as RunId;
};

/** Wiring assembled by {@link GraphBuilder.compile} and handed to a {@link CompiledGraph}. */
export type CompiledGraphParts = {
  definition: GraphDefinition;
  handlers: Map<string, NodeHandler>;
  conditions: Map<string, ConditionFn>;
  /**
   * Per agent node, the serializable config + JS tool executes the Rust engine bridge
   * needs (see {@link RustAgentConfig}). Empty for graphs with no agent nodes.
   */
  agentConfigs?: Map<string, RustAgentConfig>;
  /**
   * Per agent node, the principal that requests approvals and the node's gated tool names.
   * {@link CompiledGraph.approveAndResume} uses it to stamp each granted tool's `requestedBy`
   * for the engine's no-self-approval check. Empty for graphs with no agent nodes.
   */
  agentApprovals?: Map<string, AgentApprovalBinding>;
  /**
   * Per component node, the `{ kind, params }` carrier the Rust engine bridge needs to
   * run the native component handler (see {@link RustComponentConfig}). Empty for
   * graphs with no component nodes.
   */
  componentConfigs?: Map<string, RustComponentConfig>;
  /**
   * Per `mapAgents` node, the dynamic-fan-out carrier (over-channel, join channel, the sub-agent
   * config) the Rust bridge needs (ADR 0027 phase 4b). Empty for graphs with no mapAgents nodes.
   */
  mapAgentConfigs?: Map<string, RustMapAgentConfig>;
  /**
   * Child graphs that `subgraph`-type nodes resolve into (their node handlers /
   * conditions / agent / component configs are already merged into the maps above, by
   * global node id). Carried to the Rust engine as `EngineSpec.subgraphs`. Empty for
   * graphs with no subgraph nodes.
   */
  subgraphs?: GraphDefinition[];
  /**
   * Per-path filesystem permission rules (ADR 0024 phase 2b) applied run-wide to every
   * fs-enabled agent. Carried to the Rust engine as `EngineSpec.fsPolicy`. Empty/omitted
   * = fail-closed read-only everywhere.
   */
  fsPolicy?: FsPolicyRule[];
};

/** Options accepted by {@link CompiledGraph.run} / {@link CompiledGraph.stream}. */
export type RunOptions = {
  /** Provide a stable run id (e.g. to correlate with an external system). */
  runId?: RunId;
  /**
   * Pre-queue dynamic-message inputs (`send`) before the run: per node id, a FIFO list
   * each consumed by that node's next execution via the reserved `__injected` channel
   * (read it with {@link import("./send.js").readInjected}). The map-reduce seam.
   */
  inbox?: Record<string, unknown[]>;
};

/**
 * `AILU_SDK_ENGINE`, read when a graph compiles. The Rust engine is the only engine:
 * - `"auto"` (default) and `"rust"`: run on the Rust engine. Under `"auto"`, a graph whose
 *   agent node sets the removed `approvalEngine` option fails to compile instead.
 * - `"ts"`: removed; compiling fails with {@link RustEngineRequiredError}.
 */
type EnginePreference = "auto" | "rust" | "ts";

const enginePreference = (): EnginePreference => {
  const raw = process.env.AILU_SDK_ENGINE?.trim().toLowerCase();
  return raw === "rust" || raw === "ts" ? raw : "auto";
};

/**
 * Thrown at compile time when the graph cannot run on the **Rust engine**. The SDK is a thin
 * surface over the native engine and has **no TypeScript fallback** — it never silently degrades.
 */
export class RustEngineRequiredError extends AiluSdkError {
  public constructor(preference: EnginePreference) {
    const reason =
      preference === "ts"
        ? "AILU_SDK_ENGINE=ts is no longer supported — the TypeScript engine fallback has been removed."
        : "the native engine (@ailu-ai/napi) is not loaded, or this graph uses a feature only the " +
          "removed TS engine implemented (an ApprovalEngine-backed agent node, a JS handler returning a " +
          "routing Command `{ goto }`, or a `requiresApproval` tool node that suspends).";
    super(
      `Ailu requires the Rust engine; there is no TypeScript fallback. Cannot run this graph: ${reason} ` +
        "Build the native addon (scripts/build-napi.sh) / install @ailu-ai/napi, and use channel-based " +
        "routing/approvals.",
      {
        code: "AILU_RUST_ENGINE_REQUIRED",
        hint: "Install @ailu-ai/napi (it ships prebuilt for common platforms) — or, if the graph uses a removed TS-only feature, switch to channel-based routing/approvals."
      }
    );
    this.name = "RustEngineRequiredError";
  }
}

/**
 * A validated, runnable graph. Holds the engine wiring (registries, checkpointer, event bus) so
 * callers don't touch the lower-level `@ailu-ai/graph-runtime` primitives unless they want to.
 *
 * Execution runs **exclusively on the Rust engine** via `@ailu-ai/napi` (a required dependency).
 * There is **no TypeScript fallback** — {@link CompiledGraph} throws {@link RustEngineRequiredError}
 * at compile time if the native engine cannot run the graph.
 */
export class CompiledGraph<TState extends ChannelValues = ChannelValues> {
  public readonly definition: GraphDefinition;
  private readonly checkpointer: Checkpointer;
  private readonly eventBus: EventBus;
  private readonly runtime: GraphRuntime;

  /**
   * The Rust runner. Never `null` once constructed (the constructor throws otherwise).
   * Typed on `ChannelValues` (not `TState`) on purpose: the runner round-trips state as
   * serialized `GraphState`/JSON, so it never needs the precise channel shape — and
   * keeping `TState` out of every *field* keeps `CompiledGraph<TState>` variance-friendly
   * (a `CompiledGraph<Specific>` stays assignable to `CompiledGraph<ChannelValues>`,
   * e.g. when stored in a heterogeneous registry). The public methods re-narrow to
   * `TState` at their boundary.
   */
  private readonly rustRunner: RustGraphRunner<ChannelValues> | null;
  /** The last suspended state seen per run id, fed back into the Rust resume/approve. */
  private readonly suspendedStates = new Map<string, GraphState>();
  /** Per agent node, the approval principal used by {@link approveAndResume}. */
  private readonly agentApprovals: Map<string, AgentApprovalBinding>;

  public constructor(parts: CompiledGraphParts) {
    this.definition = parts.definition;
    this.agentApprovals = parts.agentApprovals ?? new Map<string, AgentApprovalBinding>();

    const nodeRegistry = new InMemoryNodeRegistry();
    for (const [nodeId, handler] of parts.handlers) {
      nodeRegistry.register(nodeId as NodeId, handler);
    }

    const conditionRegistry = new InMemoryConditionRegistry();
    for (const [name, fn] of parts.conditions) {
      conditionRegistry.register(name, fn);
    }

    this.checkpointer = new InMemoryCheckpointer();
    this.eventBus = new InMemoryEventBus();

    // The in-process runtime behind the deprecated `engine` getter. It does not run this
    // graph; the Rust runner below does.
    const subgraphsById = new Map<string, GraphDefinition>(
      (parts.subgraphs ?? []).map((graph) => [String(graph.id), graph])
    );

    this.runtime = new GraphRuntime({
      graph: parts.definition,
      nodeRegistry,
      conditionRegistry,
      checkpointer: this.checkpointer,
      eventBus: this.eventBus,
      subgraphResolver:
        subgraphsById.size === 0
          ? undefined
          : (graphId) => subgraphsById.get(String(graphId))
    });

    this.rustRunner = this.maybeCreateRustRunner(parts);
    // Rust-only: the SDK is a thin surface over the Rust engine — there is NO TypeScript
    // fallback. If the native engine cannot run this graph (the napi addon is absent, or the
    // graph uses a feature that only the deprecated TS engine implemented), fail loudly rather
    // than silently degrade. See RustEngineRequiredError for the reasons + remedy.
    if (this.rustRunner === null) {
      throw new RustEngineRequiredError(enginePreference());
    }
    // Mirror forwarded run events into the event bus so `onEvent` subscribers see them.
    this.rustRunner.subscribe((event) => {
      this.eventBus.emit(event);
    });
  }

  /**
   * Build the Rust runner, or return `null` when the graph can't run: the native addon is
   * missing, `AILU_SDK_ENGINE=ts` is set, or (under `auto`) an agent node sets the removed
   * `approvalEngine` option. The constructor turns `null` into {@link RustEngineRequiredError}.
   *
   * Agents run natively with the engine's own gateway (the `llm` option is ignored). JS node
   * handlers and tool `execute` functions are called over the napi seam, which awaits them. A
   * handler returning a routing `Command` (`{ goto }`) has its `goto` ignored: route with a
   * conditional edge instead.
   */
  private maybeCreateRustRunner(parts: CompiledGraphParts): RustGraphRunner<ChannelValues> | null {
    const preference = enginePreference();
    if (preference === "ts" || !rustEngineAvailable()) {
      return null;
    }

    const agentConfigs = parts.agentConfigs ?? new Map<string, RustAgentConfig>();
    const componentConfigs = parts.componentConfigs ?? new Map<string, RustComponentConfig>();
    const mapAgentConfigs = parts.mapAgentConfigs ?? new Map<string, RustMapAgentConfig>();

    // An agent node's `approvalEngine` option belonged to the removed TypeScript engine: refuse
    // the graph rather than silently skip its approvals. `rust` overrides this.
    if (preference === "auto") {
      const usesApprovalEngine = [...agentConfigs.values()].some((config) => config.usesApprovalEngine);
      if (usesApprovalEngine) {
        return null;
      }
    }

    const jsHandlerNodeIds = new Set(parts.handlers.keys());
    // Agent nodes also have an entry in `parts.handlers`, but they run natively, so they are
    // not JS node ids. Any *other* handler is a JS action/custom/tool node.
    for (const agentNodeId of agentConfigs.keys()) {
      jsHandlerNodeIds.delete(agentNodeId);
    }
    // Component nodes likewise run their native Rust handler (the bridge routes a
    // `componentNodes` entry before the JS seam), so they are not JS node ids either.
    for (const componentNodeId of componentConfigs.keys()) {
      jsHandlerNodeIds.delete(componentNodeId);
    }

    // JS tool `execute` fns are async and bridge by default (the seam awaits them).
    const toolFns = this.buildToolFns(agentConfigs);

    const runnerParts: RustRunnerParts<ChannelValues> = {
      definition: parts.definition,
      subgraphs: parts.subgraphs ?? [],
      nodeFns: this.buildNodeFns(jsHandlerNodeIds, parts.handlers),
      toolFns,
      conditions: this.buildConditionFns(parts.conditions),
      agents: agentConfigs,
      components: componentConfigs,
      mapAgents: mapAgentConfigs,
      jsNodeIds: jsHandlerNodeIds,
      jsToolNames: new Set(toolFns.keys()),
      fsPolicy: parts.fsPolicy ?? []
    };
    return tryCreateRustRunner<ChannelValues>(runnerParts);
  }

  /**
   * Adapt the (async) JS node handlers into the async producers the Rust seam needs.
   * The Rust side awaits the returned promise, so a handler doing real async work
   * round-trips faithfully. A handler that returns a routing {@link Command} keeps only
   * its `update` map (see {@link toUpdateObject}): the engine routes by the graph's edges.
   */
  private buildNodeFns(
    jsNodeIds: Set<string>,
    handlers: Map<string, NodeHandler>
  ): Map<string, AsyncNodeFn<ChannelValues>> {
    const out = new Map<string, AsyncNodeFn<ChannelValues>>();
    for (const nodeId of jsNodeIds) {
      const handler = handlers.get(nodeId);
      if (handler === undefined) {
        continue;
      }
      out.set(nodeId, async (state) => {
        // Match the TS runtime's call convention: the first arg is the channel map
        // (`handler(state.channels, state, ctx)` in graph-runtime). Some handlers —
        // notably the tool node from `createToolNode` — read their channels from this
        // `input` arg, not `state.channels`, so passing `null` would break them.
        const result = await handler(state.channels, state as unknown as GraphState, syntheticContext());
        return toUpdateObject(result);
      });
    }
    return out;
  }

  /** Async tool executes for every JS-backed tool across all agent nodes. */
  private buildToolFns(agentConfigs: Map<string, RustAgentConfig>): Map<string, AsyncToolFn> {
    const out = new Map<string, AsyncToolFn>();
    for (const config of agentConfigs.values()) {
      for (const binding of config.toolBindings) {
        out.set(binding.name, (input) => binding.execute(input));
      }
    }
    return out;
  }

  /** The named condition predicates, retyped for the Rust seam (already synchronous). */
  private buildConditionFns(
    conditions: Map<string, ConditionFn>
  ): Map<string, (state: TypedGraphState<ChannelValues>) => boolean> {
    const out = new Map<string, (state: TypedGraphState<ChannelValues>) => boolean>();
    for (const [name, fn] of conditions) {
      out.set(name, (state) => fn(state as unknown as GraphState));
    }
    return out;
  }

  /** True when this graph executes on the Rust engine. */
  public get usesRustEngine(): boolean {
    return this.rustRunner !== null;
  }

  /** Start a fresh run from the entry node and execute until completion or suspension. */
  public async run(
    initialData: InitialData<TState> = {} as InitialData<TState>,
    options?: RunOptions
  ): Promise<TypedGraphState<TState>> {
    const runId = options?.runId ?? generateRunId();
    const state = await this.rustRunner!.run(
      runId,
      initialData as Record<string, unknown>,
      options?.inbox ?? {}
    );
    this.captureSuspension(state);
    return state as unknown as TypedGraphState<TState>;
  }

  /** Resume a previously suspended run from its latest checkpoint. */
  public async resume(runId: RunId): Promise<TypedGraphState<TState>> {
    const suspended = this.requireSuspendedState(runId);
    const state = await this.rustRunner!.resume(suspended);
    this.captureSuspension(state);
    return state as unknown as TypedGraphState<TState>;
  }

  /**
   * Grant approval for the named tools and resume a run that suspended for approval
   * (an agent node with `suspendForApproval`). The approved tools are written into
   * `__approvedTools` by the engine before resuming, so the agent re-runs and executes
   * the now-approved tools instead of gating them again. An agent never approves its own
   * tools; this is the human seam.
   */
  public async approveAndResume(
    runId: RunId,
    options: ApproveAndResumeOptions
  ): Promise<TypedGraphState<TState>> {
    const resolvedBy = typeof options.resolvedBy === "string" ? options.resolvedBy.trim() : "";
    if (resolvedBy === "") {
      throw new ApproverRequiredError();
    }
    const suspended = this.requireSuspendedState(runId);
    const wire = this.toApprovedToolWire(options.approvedTools, resolvedBy);
    const state = await this.rustRunner!.approveAndResume(suspended, wire);
    this.captureSuspension(state);
    return state as unknown as TypedGraphState<TState>;
  }

  /**
   * Deliver an external signal to a run suspended on a `waitForSignal` node, then
   * resume it: the payload is injected into the `__signals` channel under `name` and
   * the run advances past the waiting node. The seam a control plane uses to wake a
   * run on an external event (a webhook, a message, an approval-out-of-band).
   *
   * Durable timers + external signals run natively on the **Rust engine** (the only
   * runtime). The seam a control plane uses to wake a run on an external event.
   */
  public async signal(
    runId: RunId,
    name: string,
    payload?: unknown
  ): Promise<TypedGraphState<TState>> {
    const suspended = this.requireSuspendedState(runId);
    const state = await this.rustRunner!.signal(suspended, name, payload);
    this.captureSuspension(state);
    return state as unknown as TypedGraphState<TState>;
  }

  /**
   * Project granted tool names into the wire shape the Rust engine validates: each
   * tool carries the principal that requested it (the owning agent node) and the
   * distinct principal granting it. Names are sorted so the wire payload is
   * deterministic regardless of the caller's ordering.
   */
  private toApprovedToolWire(
    approvedTools: Array<string | { name: string; key?: string }>,
    resolvedBy: string
  ): ApprovedToolWire[] {
    return approvedTools
      .map((grant) => (typeof grant === "string" ? { name: grant } : grant))
      .sort((a, b) => (a.key ?? a.name).localeCompare(b.key ?? b.name))
      .map((grant) => ({
        name: grant.name,
        key: grant.key,
        requestedBy: this.requesterOfTool(grant.name),
        resolvedBy
      }));
  }

  /** The agent node that declared `toolName` as approval-gated, as the request principal. */
  private requesterOfTool(toolName: string): string {
    for (const binding of this.agentApprovals.values()) {
      if (binding.approvalToolNames.includes(toolName)) {
        return binding.requestedBy;
      }
    }
    // No owning agent on record (e.g. a channel-only grant): fall back to the first
    // agent's requester, else a neutral principal. The Rust guard-rail still rejects a
    // resolver equal to whatever requester we report, so the invariant is preserved.
    const first = this.agentApprovals.values().next().value as AgentApprovalBinding | undefined;
    return first?.requestedBy ?? "agent";
  }

  /**
   * Stream events as the graph executes. See {@link StreamMode} for the available
   * shapes. The modes are projected, incrementally, over the run-event feed that crosses
   * napi:
   * - `updates` — a `state_update` per node completion (`delta` = the node's output).
   * - `values` — a full `state_value` per node completion, accumulated by replaying the
   *   node deltas through the channel reducers (the SDK mirrors the engine's reducers),
   *   plus a final authoritative `state_value` from the resolved run.
   * - `messages` — a `message_delta` per new entry appended to the `messages` channel,
   *   plus (ADR 0033, when the run opts into `streamTokens`) a `message_delta` per LLM
   *   token delta, grouped by `messageId`.
   * - `debug` — every run-lifecycle event wrapped as a `debug` payload.
   */
  public stream(
    initialData: InitialData<TState>,
    mode: StreamMode,
    options?: RunOptions
  ): AsyncIterable<StreamEvent> {
    const runId = options?.runId ?? generateRunId();
    return this.streamViaRust(runId, initialData, mode);
  }

  /** Seed a running channel map from the graph's channel defaults + the run's input. */
  private seedChannels(initialData: InitialData<TState>): Record<string, unknown> {
    const running: Record<string, unknown> = {};
    for (const [name, def] of Object.entries(this.definition.channels)) {
      running[name] = (initialData as Record<string, unknown>)[name] ?? def.default ?? null;
    }
    for (const [key, value] of Object.entries(initialData as Record<string, unknown>)) {
      if (!(key in running)) {
        running[key] = value;
      }
    }
    return running;
  }

  /** Apply a node delta to the running channels via the declared reducers (engine parity). */
  private applyDelta(running: Record<string, unknown>, delta: Record<string, unknown>): void {
    for (const [key, value] of Object.entries(delta)) {
      const reducer = this.definition.channels[key]?.reducer ?? "replace";
      if (reducer === "append") {
        const current = running[key];
        const existing = Array.isArray(current) ? current : current == null ? [] : [current];
        running[key] = Array.isArray(value) ? [...existing, ...value] : [...existing, value];
      } else if (reducer === "merge" && value !== null && typeof value === "object" && !Array.isArray(value)) {
        const current = running[key];
        const base =
          current !== null && typeof current === "object" && !Array.isArray(current)
            ? (current as Record<string, unknown>)
            : {};
        running[key] = { ...base, ...(value as Record<string, unknown>) };
      } else {
        running[key] = value;
      }
    }
  }

  /**
   * Drive the Rust run and project its forwarded run-event feed into {@link StreamEvent}s,
   * incrementally for every mode. Events arrive via the runner's subscriber while the run
   * promise is in flight; a small wake/queue interleaves them with the run's completion.
   */
  private async *streamViaRust(
    runId: RunId,
    initialData: InitialData<TState>,
    mode: StreamMode
  ): AsyncIterable<StreamEvent> {
    const queue: StreamEvent[] = [];
    let notify: (() => void) | null = null;
    const wake = (): void => {
      const resume = notify;
      notify = null;
      resume?.();
    };

    // For `values`/`messages` we accumulate channel state across node deltas.
    const running = this.seedChannels(initialData);
    let seenMessages = Array.isArray(running.messages) ? running.messages.length : 0;

    const shape = (event: RunEvent): StreamEvent[] => {
      if (event.type === "node_completed") {
        const delta = (event.output ?? {}) as Record<string, unknown>;
        if (mode === "updates") {
          return [{ type: "state_update", delta, nodeId: event.nodeId }];
        }
        this.applyDelta(running, delta);
        if (mode === "values") {
          return [{ type: "state_value", state: this.syntheticState(runId, event.nodeId, running) }];
        }
        if (mode === "messages") {
          const messages = Array.isArray(running.messages) ? running.messages : [];
          const fresh = messages.slice(seenMessages);
          seenMessages = messages.length;
          return fresh.flatMap((message) => messageDeltas(message, event.nodeId));
        }
      }
      // ADR 0033 phase 13: per-token LLM deltas project onto the existing `message_delta`
      // stream event (no new StreamMode). `messageId` groups a turn's tokens so the client
      // concatenates them; `nodeId` is the agent node (or the `mapAgents` node for a spawn).
      if (event.type === "token_delta" && mode === "messages") {
        return [
          {
            type: "message_delta",
            delta: event.delta,
            nodeId: event.nodeId,
            messageId: event.messageId
          }
        ];
      }
      if (mode === "debug") {
        const nodeId = ("nodeId" in event ? event.nodeId : ("" as NodeId)) as NodeId;
        return [{ type: "debug", payload: event, nodeId }];
      }
      return [];
    };

    // The runner is shared by every run of this compiled graph, so its subscribers see the
    // events of all in-flight runs. Keep only this run's events and those of its child runs
    // (subgraph / map children are `<runId>:<nodeId>[:<index>]`).
    const childPrefix = `${runId}:`;
    const unsubscribe = this.rustRunner!.subscribe((event) => {
      if (event.runId !== runId && !event.runId.startsWith(childPrefix)) {
        return;
      }
      const shaped = shape(event);
      if (shaped.length > 0) {
        queue.push(...shaped);
        wake();
      }
    });

    let done = false;
    // ADR 0033 phase 13: `messages` mode is the token-granular consumer, so it opts the
    // run into per-token streaming. Other modes leave it off → agents stay on `complete()`.
    const runPromise = this.rustRunner!.run(
      runId,
      initialData as Record<string, unknown>,
      {},
      mode === "messages"
    )
      .then((state) => {
        this.captureSuspension(state);
        return state;
      })
      .finally(() => {
        done = true;
        wake();
      });
    // The rejection is re-thrown to the consumer by the `await runPromise` below. Until then the
    // generator may sit suspended at a `yield`, or the consumer may stop iterating early and never
    // reach that await — mark the promise handled so neither case is an unhandled rejection.
    runPromise.catch(() => undefined);

    try {
      for (;;) {
        while (queue.length > 0) {
          yield queue.shift() as StreamEvent;
        }
        if (done) {
          break;
        }
        await new Promise<void>((resolve) => {
          notify = resolve;
        });
      }
      const finalState = await runPromise;
      // A final authoritative snapshot (carries engine-internal channels the accumulator
      // can't see, e.g. __suspend) — only for `values`.
      if (mode === "values") {
        yield { type: "state_value", state: finalState as unknown as GraphState };
      }
    } finally {
      unsubscribe();
    }
  }

  /** A channels-only synthetic GraphState for a `values` stream step. */
  private syntheticState(
    runId: RunId,
    nodeId: NodeId,
    channels: Record<string, unknown>
  ): GraphState {
    return {
      runId,
      graphId: this.definition.id,
      currentNodeId: nodeId,
      status: "running",
      channels: { ...channels },
      version: 0,
      createdAt: "",
      updatedAt: ""
    };
  }

  /** Subscribe to the run-event lifecycle stream. Returns an unsubscribe function. */
  public onEvent(handler: (event: RunEvent) => void): () => void {
    return this.eventBus.subscribe(handler);
  }

  /**
   * @deprecated The in-process TypeScript runtime. It does not run this graph: the Rust engine
   * does, through `run` / `resume` / `approveAndResume` / `signal` / `stream`.
   */
  public get engine(): GraphRuntime {
    return this.runtime;
  }

  /**
   * Explain where a run stands (ADR AI-DX): its status, why it suspended, what unblocks it, and
   * what failed — a structured account a human or an AI agent reads to pick the next move. Reads
   * the suspended state this instance retains; an unknown run id (it completed, failed, or runs on
   * another instance) returns a `status: "unknown"` explanation. For an arbitrary run, pass its
   * `GraphState` to {@link explainRun} directly.
   */
  public explain(runId: RunId): RunExplanation {
    const state = this.suspendedStates.get(String(runId));
    if (state === undefined) {
      return {
        runId: String(runId),
        status: "unknown",
        currentNode: "",
        summary: `No suspended run '${String(runId)}' on this CompiledGraph instance — it has completed, failed, or runs elsewhere. Pass a live GraphState to explainRun(state) to explain any run.`,
        channels: []
      };
    }
    return explainRun(state);
  }

  /** Record a run's state if it suspended, so resume/approve can feed it back to Rust. */
  private captureSuspension(state: TypedGraphState<ChannelValues>): void {
    if (state.status === "suspended") {
      this.suspendedStates.set(String(state.runId), state as unknown as GraphState);
    } else {
      this.suspendedStates.delete(String(state.runId));
    }
  }

  private requireSuspendedState(runId: RunId): GraphState {
    const state = this.suspendedStates.get(String(runId));
    if (state === undefined) {
      throw new ResumeStateNotFoundError(String(runId));
    }
    return state;
  }
}

/**
 * A minimal {@link NodeExecutionContext} for the Rust seam. The channels-only state
 * the seam delivers carries no memory store, and SDK node handlers reached on the
 * Rust path read `state.channels` only; this satisfies the handler signature without
 * pulling a real store across the boundary.
 */
const syntheticContext = (): NodeExecutionContext =>
  ({ memory: undefined as unknown as NodeExecutionContext["memory"] }) satisfies NodeExecutionContext;

/**
 * Project a `messages`-channel entry into stream events for the `messages` mode: a
 * `message_delta` for string content, and a `tool_call` per tool call. Message-level
 * (one delta per whole message) — token-level deltas need gateway token streaming.
 */
const messageDeltas = (message: unknown, nodeId: NodeId): StreamEvent[] => {
  if (message === null || typeof message !== "object") {
    return [];
  }
  const msg = message as { id?: unknown; content?: unknown; toolCalls?: unknown };
  const out: StreamEvent[] = [];
  const messageId = typeof msg.id === "string" ? msg.id : "";
  if (typeof msg.content === "string" && msg.content.length > 0) {
    out.push({ type: "message_delta", delta: msg.content, nodeId, messageId });
  }
  if (Array.isArray(msg.toolCalls)) {
    for (const call of msg.toolCalls) {
      if (call !== null && typeof call === "object") {
        const c = call as { id?: unknown; name?: unknown; args?: unknown; input?: unknown };
        out.push({
          type: "tool_call",
          toolId: typeof c.id === "string" ? c.id : typeof c.name === "string" ? c.name : "",
          input: c.args ?? c.input ?? null,
          nodeId
        });
      }
    }
  }
  return out;
};

/**
 * Coerce a resolved node-handler result into the channel-update map the Rust seam
 * applies. A plain object is the update directly. A routing {@link Command}
 * (`{ goto, update? }`) contributes only its `update` map: the engine routes by the
 * graph's edges, so an in-handler `goto` is ignored (use a conditional edge). `null` /
 * primitives yield an empty update, matching Rust's tolerant `parse_update`.
 */
const toUpdateObject = (value: unknown): Record<string, unknown> => {
  if (value === null || typeof value !== "object") {
    return {};
  }
  const maybeCommand = value as { goto?: unknown; update?: unknown };
  if (maybeCommand.goto !== undefined) {
    return maybeCommand.update !== null && typeof maybeCommand.update === "object"
      ? (maybeCommand.update as Record<string, unknown>)
      : {};
  }
  return value as Record<string, unknown>;
};
