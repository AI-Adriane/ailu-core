import {
  validateGraph,
  type ChannelDefinition,
  type ChannelReducer,
  type EdgeDefinition,
  type EdgeId,
  type GraphDefinition,
  type GraphId,
  type Message,
  type NodeDefinition,
  type NodeId,
  type NodeType,
  type RetryPolicy
} from "@ailu-ai/graph-core";
import type { AgentResult } from "@ailu-ai/agents-core";
import type { ConditionFn, NodeHandler } from "@ailu-ai/graph-runtime";

import { CompiledGraph } from "./compiled-graph.js";
import {
  APPROVAL_IDS_CHANNEL,
  APPROVED_TOOLS_CHANNEL,
  createAgentNodeHandler,
  createToolNodeHandler,
  DEFAULT_AGENT_OUTPUT_CHANNEL,
  toAgentApprovalBinding,
  toRustAgentConfig,
  type AgentApprovalBinding,
  type AgentNodeConfig,
  type FsPolicyRule,
  type MapAgentNodeConfig,
  type RustAgentConfig,
  type RustMapAgentConfig,
  type TaskNodeConfig,
  type ToolNodeConfig
} from "./agent-node.js";
import type { ComponentDescriptor, RustComponentConfig } from "./components.js";
import {
  DuplicateNodeError,
  GraphCompileError,
  MissingHandlerError,
  UnknownNodeError,
  type Result
} from "./errors.js";
import { tryRustValidate } from "./rust-validator.js";
import type { ChannelValues, EmptyChannels, TypedCondition, TypedNodeHandler } from "./typed.js";

/** Options passed to {@link createGraph}. */
export type CreateGraphOptions = {
  name: string;
  /** Defaults to a slugified `name`. */
  id?: string;
  /** Semver-ish version string. Defaults to `"0.0.0"`. */
  version?: string;
  recursionLimit?: number;
  metadata?: Record<string, unknown>;
};

/** Channel shorthand: `reducer` defaults to `"replace"`. The value type is inferred from `default`. */
export type ChannelInput<TValue = unknown> = {
  type: string;
  reducer?: ChannelReducer;
  default?: TValue;
  /** ADR 0032: never emit this channel's value in run events/logs (masked; still checkpointed). */
  noLog?: boolean;
};

/** Config form for non-trivial nodes. A bare handler is the common case. */
export type NodeInput<TState extends ChannelValues> = {
  type?: NodeType;
  handler?: TypedNodeHandler<TState>;
  label?: string;
  retryPolicy?: RetryPolicy;
  metadata?: Record<string, unknown>;
};

const slugify = (value: string): string =>
  value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "") || "graph";

/**
 * Fluent builder for an Ailu graph. Add channels, nodes and edges, then
 * {@link GraphBuilder.compile} into a runnable {@link CompiledGraph}.
 *
 * The `TState` type parameter accumulates the declared channels as you call
 * `.channel(...)`, so handler state and the result of `run`/`resume` are fully
 * typed without any manual annotation.
 *
 * Conditions are always **named predicates** registered here — never `eval`'d
 * strings — which is what keeps conditional routing safe and inspectable.
 */
export class GraphBuilder<TState extends ChannelValues = EmptyChannels> {
  private readonly options: CreateGraphOptions;
  private readonly channels: Record<string, ChannelDefinition<unknown>> = {};
  private readonly nodes: NodeDefinition[] = [];
  private readonly edges: EdgeDefinition[] = [];
  private readonly handlers = new Map<string, NodeHandler>();
  private readonly conditions = new Map<string, ConditionFn>();
  /** Per agent node, the serializable config the Rust engine bridge needs. */
  private readonly agentConfigs = new Map<string, RustAgentConfig>();
  private readonly mapAgentConfigs = new Map<string, RustMapAgentConfig>();
  /** Per agent node, the governance binding (approval engine + requester) for resume. */
  private readonly agentApprovals = new Map<string, AgentApprovalBinding>();
  /** Per component node, the `{ kind, params }` carrier the Rust engine bridge needs. */
  private readonly componentConfigs = new Map<string, RustComponentConfig>();
  /** Child graphs registered as `subgraph` nodes, keyed by their (global) graph id. */
  private readonly subgraphDefs = new Map<string, GraphDefinition>();
  /** Per-path filesystem permission rules (ADR 0024 phase 2b), applied run-wide. */
  private readonly fsPolicyRules: FsPolicyRule[] = [];
  private entryNodeId: string | undefined;

  public constructor(options: CreateGraphOptions) {
    this.options = options;
  }

  /** Reinterpret `this` under a wider channel type after declaring a new channel. */
  private widen<TNext extends ChannelValues>(): GraphBuilder<TNext> {
    return this as unknown as GraphBuilder<TNext>;
  }

  /** Declare a state channel. `reducer` defaults to `"replace"`; value type inferred from `default`. */
  public channel<TName extends string, TValue = unknown>(
    name: TName,
    definition: ChannelInput<TValue>
  ): GraphBuilder<TState & { [K in TName]: TValue }> {
    this.channels[name] = {
      type: definition.type,
      reducer: definition.reducer ?? "replace",
      default: definition.default,
      ...(definition.noLog === true ? { noLog: true } : {})
    };
    return this.widen<TState & { [K in TName]: TValue }>();
  }

  /** Declare an append-reduced `messages` channel (the conversational default). */
  public messagesChannel<TName extends string = "messages">(
    name: TName = "messages" as TName
  ): GraphBuilder<TState & { [K in TName]: Message[] }> {
    this.channels[name] = { type: "messages", reducer: "append", default: [] };
    return this.widen<TState & { [K in TName]: Message[] }>();
  }

  /** Shared node-registration path: dedupe, register the handler, default the entry. */
  private pushNode(
    id: string,
    type: NodeType,
    label: string,
    handler?: NodeHandler,
    extras?: { retryPolicy?: RetryPolicy; metadata?: Record<string, unknown> }
  ): void {
    if (this.handlers.has(id) || this.nodes.some((node) => String(node.id) === id)) {
      throw new DuplicateNodeError(id);
    }
    if (handler !== undefined) {
      this.handlers.set(id, handler);
    }
    this.nodes.push({
      id: id as NodeId,
      type,
      label,
      retryPolicy: extras?.retryPolicy,
      metadata: extras?.metadata
    });
    this.entryNodeId ??= id;
  }

  /** Declare a channel only if it hasn't been declared yet (for node helpers that need one). */
  private ensureChannel(name: string, definition: ChannelDefinition<unknown>): void {
    if (!(name in this.channels)) {
      this.channels[name] = definition;
    }
  }

  /** Add a node. Pass a handler for the common action case, or a config object. */
  public node(id: string, handlerOrConfig: TypedNodeHandler<TState> | NodeInput<TState>): this {
    const config: NodeInput<TState> =
      typeof handlerOrConfig === "function" ? { type: "action", handler: handlerOrConfig } : handlerOrConfig;

    const type: NodeType = config.type ?? "action";
    if (type === "action" && config.handler === undefined) {
      throw new MissingHandlerError(id);
    }

    this.pushNode(id, type, config.label ?? id, config.handler as NodeHandler | undefined, {
      retryPolicy: config.retryPolicy,
      metadata: config.metadata
    });
    return this;
  }

  /** Convenience for a `human-gate` node that suspends the run for approval. */
  public humanGate(id: string, options?: { label?: string }): this {
    this.pushNode(id, "human-gate", options?.label ?? id);
    return this;
  }

  /**
   * Add an agent node: a ReAct agent driven by an LLM gateway. Its result lands in
   * `config.outputChannel` (default `"agentResult"`), which is auto-declared and
   * added to the typed state. Route on `result.requiresHumanReview` to gate
   * sensitive tool use.
   */
  public agentNode<TOut extends string = typeof DEFAULT_AGENT_OUTPUT_CHANNEL>(
    id: string,
    config: AgentNodeConfig & { outputChannel?: TOut }
  ): GraphBuilder<TState & { [K in TOut]: AgentResult }> {
    const outputChannel = config.outputChannel ?? DEFAULT_AGENT_OUTPUT_CHANNEL;
    // Capture the serializable projection so the Rust engine bridge can run this agent
    // node natively; the TS handler keeps the TS-engine path working.
    const rustConfig = toRustAgentConfig(id, config);
    // Also emit the SHARED CARRIER on `node.metadata.agent` (the wire-serializable
    // fields only — no LLM gateway, no tool closures) so the persisted GraphDefinition
    // is executable by the control plane's catalog run path and renderable in Studio.
    this.pushNode(id, "agent", config.label ?? id, createAgentNodeHandler(id, config), {
      metadata: {
        agent: {
          provider: rustConfig.provider,
          model: rustConfig.model,
          tier: rustConfig.tier,
          system: rustConfig.system,
          toolNames: rustConfig.toolNames,
          maxIterations: rustConfig.maxIterations,
          suspendForApproval: rustConfig.suspendForApproval,
          approvalToolNames: rustConfig.approvalToolNames,
          outputChannel: rustConfig.outputChannel,
          // ADR 0014 (terse/trim) + ADR 0022/0023 (durable todos channel) + ADR 0024
          // (fs enablement): carried so the persisted GraphDefinition runs identically on
          // the catalog/Studio path.
          outputStyle: rustConfig.outputStyle,
          contextBudget: rustConfig.contextBudget,
          todosChannel: rustConfig.todosChannel,
          inputBlocksChannel: rustConfig.inputBlocksChannel,
          memory: rustConfig.memory,
          skills: rustConfig.skills,
          enableFs: rustConfig.enableFs,
          // ADR 0025 phase 3d — the resolved efficiency middleware list (profile + explicit
          // middleware + flat knobs, already desugared) so the catalog/Studio path reaches
          // the bridge with the same stack as the in-process builder.
          resolvedMiddleware: rustConfig.resolvedMiddleware
        }
      }
    });
    this.agentConfigs.set(id, rustConfig);
    this.agentApprovals.set(id, toAgentApprovalBinding(id, config));
    this.ensureChannel(outputChannel, { type: "agentResult", reducer: "replace" });
    // Channels the control plane / ApprovalEngine use to gate and resume.
    this.ensureChannel(APPROVED_TOOLS_CHANNEL, { type: "string[]", reducer: "replace", default: [] });
    this.ensureChannel(APPROVAL_IDS_CHANNEL, { type: "string[]", reducer: "replace", default: [] });
    return this.widen<TState & { [K in TOut]: AgentResult }>();
  }

  /**
   * Add a tool node: executes the tool calls emitted by the last AI message in the
   * `messages` channel (auto-declared as an append-reduced messages channel).
   */
  public toolNode(id: string, config: ToolNodeConfig): GraphBuilder<TState & { messages: Message[] }> {
    this.pushNode(id, "tool", config.label ?? id, createToolNodeHandler(config));
    this.ensureChannel("messages", { type: "messages", reducer: "append", default: [] });
    return this.widen<TState & { messages: Message[] }>();
  }

  /**
   * Add a **component node**: a pure (no-LLM) compute building block from
   * {@link import("./components.js").components} (e.g. `promptBuilder`, `router`,
   * `retriever`). The node carries the Phase C carrier (`{ kind, params }`) so it runs
   * natively on the Rust engine, *and* registers the descriptor's equivalent TS handler
   * so the TS fallback path stays faithful when the native addon is absent.
   *
   * On the Rust path the component takes precedence over the JS seam even though its id
   * is also a JS node id — the bridge routes a `componentNodes` entry to the native
   * handler. So the node always runs the same logic on either engine.
   *
   * ```ts
   * createGraph({ name: "p" })
   *   .channel("name", { type: "string", default: "" })
   *   .channel("prompt", { type: "string", default: "" })
   *   .component("build", components.promptBuilder({ template: "Hi {{name}}", into: "prompt" }));
   * ```
   */
  public component(id: string, descriptor: ComponentDescriptor, options?: { label?: string }): this {
    // Push as an `action` node carrying the TS-equivalent handler (the TS fallback
    // path) AND the SHARED CARRIER on `node.metadata.component` so the persisted
    // GraphDefinition is executable by the control plane's catalog run path
    // (see run-catalog-graph.ts) and renderable in the Studio editor. The Rust path
    // runs the native component handler, keyed by the `componentConfigs` carrier below.
    this.pushNode(id, "action", options?.label ?? id, descriptor.handler, {
      metadata: { component: { kind: descriptor.kind, params: descriptor.params } }
    });
    this.componentConfigs.set(id, { kind: descriptor.kind, params: descriptor.params });
    return this;
  }

  /**
   * Add a **subgraph node**: nest another graph (built with its own
   * {@link GraphBuilder}) as a single node. On entry the parent's channels are
   * projected into the child via `inputMapping` (`childKey → parentKey`; omit to copy
   * all parent channels); on completion the child's channels are merged back via
   * `outputMapping` (`parentKey → childKey`; omit to spread all child channels onto
   * the parent). If the child suspends (e.g. an internal human gate), the parent
   * suspends at this node and a parent `resume` re-attaches to the child.
   *
   * The child's wiring (node handlers, conditions, agent/component configs) is merged
   * into the parent — child runs share the parent's registries, keyed by GLOBAL node
   * id — so child node ids must not collide with the parent's. Declare on the parent
   * any channels the `outputMapping` writes into.
   *
   * ```ts
   * const child = createGraph({ name: "double", id: "double" })
   *   .channel("in", { type: "number", default: 0 })
   *   .channel("out", { type: "number", default: 0 })
   *   .node("calc", (s) => ({ out: (s.in as number) * 2 }));
   * createGraph({ name: "parent" })
   *   .channel("x", { type: "number", default: 21 })
   *   .channel("y", { type: "number", default: 0 })
   *   .subgraph("sub", child, { inputMapping: { in: "x" }, outputMapping: { y: "out" } });
   * ```
   */
  public subgraph<TChild extends ChannelValues>(
    id: string,
    child: GraphBuilder<TChild>,
    options?: {
      inputMapping?: Record<string, string>;
      outputMapping?: Record<string, string>;
      label?: string;
    }
  ): this {
    if (this.handlers.has(id) || this.nodes.some((node) => String(node.id) === id)) {
      throw new DuplicateNodeError(id);
    }
    const childParts = child.toSubgraphParts();

    // Merge the child's wiring into this builder. Child runs share the parent's
    // registries (the Rust bridge and the TS runtime both look handlers up by global
    // node id), so a child node id colliding with a parent node id is a hard error.
    for (const [nodeId, handler] of childParts.handlers) {
      if (this.handlers.has(nodeId)) {
        throw new DuplicateNodeError(nodeId);
      }
      this.handlers.set(nodeId, handler);
    }
    for (const [name, fn] of childParts.conditions) {
      this.conditions.set(name, fn);
    }
    for (const [nodeId, config] of childParts.agentConfigs) {
      this.agentConfigs.set(nodeId, config);
    }
    for (const [nodeId, config] of childParts.mapAgentConfigs) {
      this.mapAgentConfigs.set(nodeId, config);
    }
    for (const [nodeId, binding] of childParts.agentApprovals) {
      this.agentApprovals.set(nodeId, binding);
    }
    for (const [nodeId, config] of childParts.componentConfigs) {
      this.componentConfigs.set(nodeId, config);
    }
    // Register the child graph + any subgraphs it nested in turn.
    this.subgraphDefs.set(String(childParts.definition.id), childParts.definition);
    for (const [graphId, definition] of childParts.subgraphDefs) {
      this.subgraphDefs.set(graphId, definition);
    }

    this.nodes.push({
      id: id as NodeId,
      type: "subgraph",
      label: options?.label ?? id,
      subgraphId: childParts.definition.id,
      inputMapping: options?.inputMapping,
      outputMapping: options?.outputMapping
    });
    this.entryNodeId ??= id;
    return this;
  }

  /**
   * Add a **task node** (ADR 0022/0023, phase 1): spawn a sub-agent in an isolated
   * context that returns a single compressed report. It is sugar over
   * {@link GraphBuilder.subgraph} — the sub-agent runs as a one-node child graph, so
   * the spawn is a real node: **checkpointed, audited, and human-gate-preserving**.
   * If the sub-agent suspends for approval, the whole run suspends and a parent
   * `resume`/`approveAndResume` re-attaches to it. No new runtime path is added.
   *
   * Isolation: only `objectiveChannel` crosses into the child (`inputMapping`), and
   * only the child's `reportChannel` crosses back (`outputMapping`) — the sub-agent
   * never sees the parent's other channels, and the parent never sees the child's
   * intermediate work. With `compress` (default), the sub-agent runs
   * `outputStyle: "terse"` so the report is a summary, not a full transcript.
   *
   * ```ts
   * createGraph({ name: "research" })
   *   .channel("objective", { type: "string", default: "" })
   *   .taskNode("dig", { subAgent: { llm, prompt: { system: "Research deeply." } } });
   *   // -> state.report : AgentResult
   * ```
   */
  public taskNode<TReport extends string = "report">(
    id: string,
    config: TaskNodeConfig & { reportChannel?: TReport }
  ): GraphBuilder<TState & { [K in TReport]: AgentResult }> {
    const objectiveChannel = config.objectiveChannel ?? "objective";
    const reportChannel = (config.reportChannel ?? "report") as TReport;
    const compress = config.compress ?? true;

    // The sub-agent runs as a one-node child graph. Its id is namespaced under the task
    // node id so it cannot collide with parent node ids when the child wiring is merged.
    const childAgentId = `${id}__agent`;
    const child = createGraph({ name: `${id}-task`, id: `${id}-task` })
      .channel(objectiveChannel, { type: "string", default: "" })
      .agentNode(childAgentId, {
        ...config.subAgent,
        outputChannel: reportChannel,
        outputStyle: compress ? "terse" : config.subAgent.outputStyle
      });

    // The parent must declare the channel the outputMapping writes into; the objective
    // source channel is ensured too (idempotent) for ergonomics.
    this.ensureChannel(objectiveChannel, { type: "string", reducer: "replace", default: "" });
    this.ensureChannel(reportChannel, { type: "agentResult", reducer: "replace" });

    this.subgraph(id, child, {
      inputMapping: { [objectiveChannel]: objectiveChannel },
      outputMapping: { [reportChannel]: reportChannel },
      label: config.label ?? id
    });
    return this.widen<TState & { [K in TReport]: AgentResult }>();
  }

  /**
   * Add a **mapAgents node** (ADR 0027 phase 4b — dynamic fan-out): run `config.subAgent` once
   * per item in the `config.overChannel` array, **concurrently**, and write the per-item results
   * — in **input order** (deterministic, resumable) — into `config.joinAt` as an array. Each spawn
   * gets one item as its `input` and shares the run's channels. If a spawn needs approval and
   * `suspendForApproval` is set, the whole map suspends; resume re-runs it.
   *
   * ```ts
   * createGraph({ name: "fanout" })
   *   .channel("items", { type: "json", default: [] })
   *   .mapAgents("research", {
   *     overChannel: "items",
   *     subAgent: { llm, prompt: { system: "Summarise the item." } },
   *     joinAt: "summaries"
   *   });
   * ```
   */
  public mapAgents<TJoin extends string>(
    id: string,
    config: MapAgentNodeConfig & { joinAt: TJoin }
  ): GraphBuilder<TState & { [K in TJoin]: AgentResult[] }> {
    // The sub-agent is projected exactly like an ordinary agent; the bridge runs it per item.
    const agent = toRustAgentConfig(`${id}__agent`, config.subAgent);
    // A native node (Rust dispatch routes it via the `mapAgents` carrier); no TS handler — the
    // SDK runs on the Rust engine. The node carries the SHARED CARRIER so the catalog/Studio path
    // can run it too.
    this.pushNode(id, "agent", config.label ?? id, undefined, {
      metadata: {
        mapAgent: {
          overChannel: config.overChannel,
          joinAt: config.joinAt,
          suspendForApproval: config.suspendForApproval === true,
          agent
        }
      }
    });
    this.mapAgentConfigs.set(id, {
      overChannel: config.overChannel,
      joinAt: config.joinAt,
      agent,
      suspendForApproval: config.suspendForApproval === true
    });
    this.ensureChannel(config.overChannel, { type: "json", reducer: "replace", default: [] });
    this.ensureChannel(config.joinAt, { type: "json", reducer: "replace", default: [] });
    this.ensureChannel(APPROVED_TOOLS_CHANNEL, { type: "string[]", reducer: "replace", default: [] });
    return this.widen<TState & { [K in TJoin]: AgentResult[] }>();
  }

  /**
   * @internal Extract this builder's wiring so it can be nested as a subgraph by a
   * parent {@link GraphBuilder.subgraph}. Returns live maps (the parent merges them);
   * not part of the public authoring API.
   */
  public toSubgraphParts(): {
    definition: GraphDefinition;
    handlers: Map<string, NodeHandler>;
    conditions: Map<string, ConditionFn>;
    agentConfigs: Map<string, RustAgentConfig>;
    mapAgentConfigs: Map<string, RustMapAgentConfig>;
    agentApprovals: Map<string, AgentApprovalBinding>;
    componentConfigs: Map<string, RustComponentConfig>;
    subgraphDefs: Map<string, GraphDefinition>;
  } {
    return {
      definition: this.buildDefinition(),
      handlers: this.handlers,
      conditions: this.conditions,
      agentConfigs: this.agentConfigs,
      mapAgentConfigs: this.mapAgentConfigs,
      agentApprovals: this.agentApprovals,
      componentConfigs: this.componentConfigs,
      subgraphDefs: this.subgraphDefs
    };
  }

  /**
   * Fan out from an existing node to a fixed set of branch nodes that run
   * **concurrently** on the Rust engine, then join at `joinAt`. Each branch executes
   * from the same pre-fan-out state snapshot and the branch updates are merged in the
   * declared `parallelTo` order (deterministic, regardless of which branch finishes
   * first — ADR 0015). The `from` node runs first (its handler/agent), then its branches
   * scatter; control resumes at `joinAt` once every branch completes.
   *
   * This is the supported way to run **N parallel LLM calls** (each branch an
   * {@link GraphBuilder.agentNode}) on the public SDK — no static edges are needed for
   * the fan-out itself (it is its own routing). `parallelTo` is a fixed set declared at
   * build time; dynamic per-item map over a runtime-sized list is a separate primitive.
   *
   * `from` must already be added; `parallelTo` and `joinAt` are validated at compile.
   */
  public fanOut(from: string, parallelTo: string[], joinAt: string): this {
    const node = this.nodes.find((candidate) => String(candidate.id) === from);
    if (node === undefined) {
      throw new UnknownNodeError(from, "fanOut(from, …)");
    }
    node.fanOut = {
      parallelTo: parallelTo.map((id) => id as NodeId),
      joinAt: joinAt as NodeId
    };
    return this;
  }

  /** Add an unconditional edge from one node to another. */
  public edge(from: string, to: string): this {
    this.edges.push({
      id: `e_${from}_${to}_${this.edges.length}` as EdgeId,
      from: from as NodeId,
      to: to as NodeId,
      type: "default"
    });
    return this;
  }

  /**
   * Add a conditional edge guarded by a named predicate. The predicate is
   * registered under `conditionName` and evaluated against the live (typed) state.
   */
  public conditionalEdge(
    from: string,
    to: string,
    conditionName: string,
    predicate: TypedCondition<TState>
  ): this {
    this.conditions.set(conditionName, predicate as ConditionFn);
    this.edges.push({
      id: `e_${from}_${to}_${this.edges.length}` as EdgeId,
      from: from as NodeId,
      to: to as NodeId,
      type: "conditional",
      condition: conditionName
    });
    return this;
  }

  /**
   * ADR 0076 — governed error branch: once `from`'s `retryPolicy` is exhausted, the run reroutes
   * to `to` instead of failing (`node_error_routed` event, `channels.__lastError` populated)
   * rather than the run terminating (`run_failed`). At most one per node (enforced at compile via
   * `validateGraph`). Implemented in BOTH the Rust engine (`crates/graph-core`/`graph-runtime`,
   * the path every catalog/native run takes) and the TS legacy `@ailu-ai/graph-runtime` —
   * this builder always compiles to Rust, so this edge type works end to end here.
   */
  public errorEdge(from: string, to: string): this {
    this.edges.push({
      id: `e_${from}_${to}_${this.edges.length}` as EdgeId,
      from: from as NodeId,
      to: to as NodeId,
      type: "error"
    });
    return this;
  }

  /** Override the entry node (defaults to the first node added). */
  public entry(nodeId: string): this {
    this.entryNodeId = nodeId;
    return this;
  }

  private buildDefinition(): GraphDefinition {
    return {
      id: (this.options.id ?? slugify(this.options.name)) as GraphId,
      version: this.options.version ?? "0.0.0",
      name: this.options.name,
      recursionLimit: this.options.recursionLimit,
      channels: this.channels,
      nodes: this.nodes,
      edges: this.edges,
      entryNodeId: (this.entryNodeId ?? "") as NodeId,
      metadata: this.options.metadata
    };
  }

  /**
   * Declare per-path filesystem permission rules (ADR 0024 phase 2b) applied run-wide
   * to every agent created with `enableFs: true`. Verbs: `deny|read|write|gate`;
   * resolution is most-specific-glob-wins, fail-closed — an unmatched path resolves to
   * `read`, so writes need an explicit `write` rule (`gate` is enforced from phase 2c).
   * Repeated calls append. `*` matches within a path segment, `**` across segments.
   *
   * ```ts
   * createGraph({ name: "deep" })
   *   .fsPolicy([{ glob: "scratch/**", verb: "write" }, { glob: "secret/**", verb: "deny" }])
   *   .agentNode("worker", { llm, prompt: { system: "..." }, enableFs: true });
   * ```
   */
  public fsPolicy(rules: FsPolicyRule[]): this {
    this.fsPolicyRules.push(...rules);
    return this;
  }

  /** Validate and compile, returning a {@link Result} instead of throwing. */
  public safeCompile(): Result<CompiledGraph<TState>, GraphCompileError> {
    const definition = this.buildDefinition();
    const subgraphs = [...this.subgraphDefs.values()];
    // Validate the parent AND every registered subgraph (each is a standalone graph),
    // via the Rust core when its native addon is present; otherwise TS.
    const errors = [definition, ...subgraphs].flatMap(
      (graph) => tryRustValidate(graph) ?? validateGraph(graph)
    );
    if (errors.length > 0) {
      return { success: false, error: new GraphCompileError(errors) };
    }

    return {
      success: true,
      data: new CompiledGraph<TState>({
        definition,
        handlers: this.handlers,
        conditions: this.conditions,
        agentConfigs: this.agentConfigs,
        mapAgentConfigs: this.mapAgentConfigs,
        agentApprovals: this.agentApprovals,
        componentConfigs: this.componentConfigs,
        subgraphs,
        fsPolicy: this.fsPolicyRules
      })
    };
  }

  /** Validate and compile into a runnable graph. Throws {@link GraphCompileError} on failure. */
  public compile(): CompiledGraph<TState> {
    const result = this.safeCompile();
    if (!result.success) {
      throw result.error;
    }
    return result.data;
  }
}

/** Entry point: start building a graph. */
export const createGraph = (options: CreateGraphOptions): GraphBuilder<EmptyChannels> =>
  new GraphBuilder<EmptyChannels>(options);
