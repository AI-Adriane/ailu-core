import type {
  ChannelDefinition,
  ChannelsSchema,
  Command,
  FailureCategory,
  GraphId,
  EdgeDefinition,
  GraphDefinition,
  GraphState,
  NodeDefinition,
  NodeId,
  RunId
} from "@ailu-ai/graph-core";

/** Mirrors `@ailu-ai/graph-core`'s `FAILURE_CATEGORIES` runtime array. Duplicated rather than
 * value-imported: every other `@ailu-ai/graph-core` import in this file is TYPE-ONLY (erased
 * at build time), so this stays the sole runtime dependency-free of graph-core's actual module —
 * the same "duplicated structurally, dependency-free" posture `graph-sdk` already uses for its
 * own `@ailu-ai/contracts` carrier types. */
const FAILURE_CATEGORY_VALUES: readonly FailureCategory[] = ["transient", "permanent", "unknown"];

import type { Checkpointer, ConditionRegistry, EventBus, NodeRegistry } from "./interfaces.js";
import { InMemoryStore } from "../../memory-store/src/in-memory-store.js";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import { checkBudget, type StepBudget } from "../../agents-core/src/step-budget.js";
import { isSwarmHandoff } from "../../agents-core/src/swarm.js";
import { InMemoryCallbackManager } from "../../callbacks/src/manager.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import {
  DynamicInterrupt,
  type InterruptConfig,
  shouldInterruptAfter,
  shouldInterruptBefore
} from "./interrupt.js";
import type { StreamEvent, StreamMode } from "./stream.js";
import { createForkRunId } from "./time-travel.js";
import { RecursionLimitError } from "./cycles.js";
import { structuralEqual } from "./equality.js";
import type { Checkpoint, CheckpointId } from "./types.js";

type GraphRuntimeDeps = {
  graph: GraphDefinition<ChannelsSchema>;
  nodeRegistry: NodeRegistry;
  conditionRegistry: ConditionRegistry;
  checkpointer: Checkpointer;
  eventBus: EventBus;
  callbackManager?: CallbackManager;
  memory?: BaseStore;
  stepBudget?: StepBudget;
  subgraphResolver?: (graphId: GraphId) => GraphDefinition | undefined;
  interruptConfig?: InterruptConfig;
};

const nowIso = (): string => new Date().toISOString();

/** ADR 0076 — duck-typed classification: a thrower (e.g. `llm-gateway` on a provider 429) can set
 * `failureCategory` on the error it throws; anything else classifies as `"unknown"`. Never throws. */
const classifyFailure = (error: unknown): FailureCategory => {
  if (
    typeof error === "object" &&
    error !== null &&
    "failureCategory" in error &&
    typeof (error as { failureCategory?: unknown }).failureCategory === "string" &&
    (FAILURE_CATEGORY_VALUES as readonly string[]).includes(
      (error as { failureCategory: string }).failureCategory
    )
  ) {
    return (error as { failureCategory: FailureCategory }).failureCategory;
  }
  return "unknown";
};

const toCheckpointId = (value: string): CheckpointId => value as CheckpointId;

const createCheckpoint = (runId: RunId, graphState: GraphState): Checkpoint => ({
  id: toCheckpointId(`${String(runId)}:${Date.now()}:${Math.random().toString(36).slice(2, 8)}`),
  runId,
  graphState,
  createdAt: nowIso()
});

const SUBGRAPH_RUNS_KEY = "__subgraphRuns";
const INTERRUPT_META_KEY = "__interruptMeta";

export class GraphRuntime {
  private readonly graph: GraphDefinition<ChannelsSchema>;
  private readonly nodeRegistry: NodeRegistry;
  private readonly conditionRegistry: ConditionRegistry;
  private readonly checkpointer: Checkpointer;
  private readonly eventBus: EventBus;
  private readonly callbackManager: CallbackManager;
  public readonly memory: BaseStore;
  public readonly stepBudget: StepBudget;
  private readonly nodeById: Map<NodeId, NodeDefinition>;
  private readonly subgraphResolver?: (graphId: GraphId) => GraphDefinition | undefined;
  private readonly interruptConfig?: InterruptConfig;
  private readonly stateHistoryByRunId = new Map<RunId, GraphState<ChannelsSchema>[]>();
  private readonly stepsByRunId = new Map<RunId, number>();
  private readonly inboxByRunId = new Map<RunId, Map<NodeId, unknown[]>>();

  public constructor(deps: GraphRuntimeDeps) {
    this.graph = deps.graph;
    this.nodeRegistry = deps.nodeRegistry;
    this.conditionRegistry = deps.conditionRegistry;
    this.checkpointer = deps.checkpointer;
    this.eventBus = deps.eventBus;
    this.callbackManager = deps.callbackManager ?? new InMemoryCallbackManager();
    this.memory = deps.memory ?? new InMemoryStore();
    this.stepBudget = deps.stepBudget ?? { maxSteps: Number.MAX_SAFE_INTEGER, currentSteps: 0 };
    this.subgraphResolver = deps.subgraphResolver;
    this.interruptConfig = deps.interruptConfig;
    this.nodeById = new Map(this.graph.nodes.map((node) => [node.id, node]));
  }

  public async start(runId: RunId, initialData: Record<string, unknown>): Promise<GraphState> {
    const timestamp = nowIso();
    const initialChannels = this.buildInitialChannels(initialData);
    let state: GraphState = {
      runId,
      graphId: this.graph.id,
      currentNodeId: this.graph.entryNodeId,
      status: "running",
      channels: initialChannels,
      version: 0,
      createdAt: timestamp,
      updatedAt: timestamp
    };

    state = await this.persistCheckpoint(state);
    await this.callbackManager.emit({
      type: "onChainStart",
      runId: String(runId),
      timestamp: nowIso(),
      input: initialData
    });
    return this.runLoop(state, false);
  }

  public async *stream(
    runId: RunId,
    initialData: Record<string, unknown>,
    mode: StreamMode
  ): AsyncIterable<StreamEvent> {
    const timestamp = nowIso();
    const initialChannels = this.buildInitialChannels(initialData);
    let state: GraphState = {
      runId,
      graphId: this.graph.id,
      currentNodeId: this.graph.entryNodeId,
      status: "running",
      channels: initialChannels,
      version: 0,
      createdAt: timestamp,
      updatedAt: timestamp
    };

    state = await this.persistCheckpoint(state);

    while (state.status === "running") {
      const nodeId = state.currentNodeId;
      const beforeChannels = { ...(state.channels as Record<string, unknown>) };
      const beforeCheckpointId = state.checkpointId;

      state = await this.executeNode(nodeId, state);

      const afterChannels = state.channels as Record<string, unknown>;
      const delta = this.computeDelta(beforeChannels, afterChannels);

      if (mode === "values") {
        yield { type: "state_value", state };
        continue;
      }

      if (mode === "updates") {
        yield { type: "state_update", delta, nodeId };
        continue;
      }

      if (mode === "messages") {
        for (const event of this.extractMessageEvents(nodeId, delta)) {
          yield event;
        }
        continue;
      }

      if (mode === "debug") {
        yield {
          type: "debug",
          nodeId,
          payload: {
            stage: "node_started",
            nodeId,
            input: beforeChannels
          }
        };
        yield {
          type: "debug",
          nodeId,
          payload: {
            stage: "node_completed",
            nodeId,
            output: delta
          }
        };
        yield {
          type: "debug",
          nodeId,
          payload: {
            stage: "state",
            state
          }
        };
        yield {
          type: "debug",
          nodeId,
          payload: {
            stage: "checkpoint",
            previousCheckpointId: beforeCheckpointId,
            checkpointId: state.checkpointId
          }
        };

        for (const toolCallEvent of this.extractToolCallEvents(nodeId, delta)) {
          yield toolCallEvent;
        }
      }
    }
  }

  public async resume(runId: RunId): Promise<GraphState> {
    const checkpoint = await this.checkpointer.load(runId);
    if (checkpoint === undefined) {
      throw new Error(`No checkpoint found for run '${String(runId)}'.`);
    }

    const suspendedNode = this.nodeById.get(checkpoint.graphState.currentNodeId);
    const interruptMeta = this.readInterruptMeta(checkpoint.graphState.channels as Record<string, unknown>);
    const shouldAdvanceFromSuspended =
      suspendedNode?.type === "human-gate" || interruptMeta?.when === "after";
    const nextNodeId =
      checkpoint.graphState.status === "suspended" && shouldAdvanceFromSuspended
        ? this.nextNode(checkpoint.graphState.currentNodeId, checkpoint.graphState)
        : checkpoint.graphState.currentNodeId;

    const resumedState: GraphState = {
      ...checkpoint.graphState,
      currentNodeId: (nextNodeId ?? checkpoint.graphState.currentNodeId) as NodeId,
      status: nextNodeId === null ? "completed" : "running",
      updatedAt: nowIso()
    };

    this.eventBus.emit({
      type: "run_resumed",
      runId,
      nodeId: resumedState.currentNodeId,
      timestamp: nowIso()
    });

    const persistedState = await this.persistCheckpoint(resumedState);
    return this.runLoop(persistedState, true);
  }

  public async executeNode(nodeId: NodeId, state: GraphState): Promise<GraphState> {
    const node = this.nodeById.get(nodeId);
    if (node === undefined) {
      throw new Error(`Node '${String(nodeId)}' is not declared in graph.`);
    }

    if (shouldInterruptBefore(this.interruptConfig, nodeId)) {
      return this.suspendRun(state, nodeId, "interrupt-before", "before");
    }

    this.consumeStepBudget();
    this.assertRecursionLimit(state.runId);

    this.eventBus.emit({
      type: "node_started",
      runId: state.runId,
      nodeId,
      timestamp: nowIso()
    });

    if (node.type === "human-gate") {
      return this.suspendRun(state, nodeId, "human-gate", "after");
    }

    if (node.type === "subgraph") {
      if (node.subgraphId === undefined || this.subgraphResolver === undefined) {
        throw new Error(`Subgraph node '${String(nodeId)}' cannot resolve subgraph.`);
      }
      const childGraph = this.subgraphResolver(node.subgraphId);
      if (childGraph === undefined) {
        throw new Error(`Subgraph '${String(node.subgraphId)}' not found.`);
      }

      const childRunId = this.getOrCreateSubgraphRunId(state, nodeId);
      const childRuntime = new GraphRuntime({
        graph: childGraph,
        nodeRegistry: this.nodeRegistry,
        conditionRegistry: this.conditionRegistry,
        checkpointer: this.checkpointer,
        eventBus: this.eventBus,
        callbackManager: this.callbackManager.createChild(["subgraph"], { parentNodeId: String(nodeId) }),
        memory: this.memory,
        stepBudget: this.stepBudget,
        subgraphResolver: this.subgraphResolver
      });

      const childInitialData = this.applyInputMapping(state.channels, node.inputMapping);
      const existingChildCheckpoint = await this.checkpointer.load(childRunId);
      const childState =
        existingChildCheckpoint === undefined
          ? await childRuntime.start(childRunId, childInitialData)
          : await childRuntime.resume(childRunId);

      if (childState.status === "failed") {
        throw new Error(`Subgraph '${String(node.subgraphId)}' failed.`);
      }

      if (childState.status === "suspended") {
        const channels = this.setSubgraphRunId(state.channels, nodeId, childRunId);
        return this.suspendRun(
          { ...state, channels, version: state.version + 1 },
          nodeId,
          "human-gate",
          "during"
        );
      }

      const mergedData = this.applyOutputMapping(
        this.setSubgraphRunId(state.channels, nodeId, childRunId),
        childState.channels,
        node.outputMapping
      );
      this.eventBus.emit({
        type: "node_completed",
        runId: state.runId,
        nodeId,
        output: childState.channels,
        timestamp: nowIso()
      });
      await this.callbackManager.emit({
        type: "onNodeEnd",
        runId: String(state.runId),
        nodeId: String(nodeId),
        timestamp: nowIso(),
        output: childState.channels
      });

      const nextNodeId = this.nextNode(nodeId, { ...state, channels: mergedData });
      const nextState: GraphState = {
        ...state,
        currentNodeId: (nextNodeId ?? nodeId) as NodeId,
        channels: mergedData,
        version: state.version + 1,
        status: nextNodeId === null ? "completed" : "running",
        updatedAt: nowIso()
      };

      return this.persistCheckpoint(nextState);
    }

    const handler = this.nodeRegistry.resolve(nodeId);
    if (handler === undefined) {
      throw new Error(`No node handler registered for '${String(nodeId)}'.`);
    }

    const retryPolicy = node.retryPolicy;
    const maxAttempts = retryPolicy?.maxAttempts ?? 1;
    const backoffMs = retryPolicy?.backoffMs ?? 0;

    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      try {
        const injected = this.consumeInjectedInput(state.runId, nodeId);
        const output = await handler(injected ?? state.channels, state, { memory: this.memory });
        this.eventBus.emit({
          type: "node_completed",
          runId: state.runId,
          nodeId,
          output,
          timestamp: nowIso()
        });
        await this.callbackManager.emit({
          type: "onNodeEnd",
          runId: String(state.runId),
          nodeId: String(nodeId),
          timestamp: nowIso(),
          output
        });

        const { goto, update } = this.resolveCommand(output as Partial<Record<string, unknown>> | Command);
        const mergedData = this.applyUpdate(state.channels, update);
        const sanitizedData =
          node.type === "agent"
            ? (() => {
                const next = { ...mergedData };
                delete next.__scratchpad;
                return next;
              })()
            : mergedData;

        const nextNodeId = this.resolveNextNode(nodeId, goto, { ...state, channels: sanitizedData });
        const nextState: GraphState = {
          ...state,
          currentNodeId: (nextNodeId ?? nodeId) as NodeId,
          channels: sanitizedData,
          version: state.version + 1,
          status: nextNodeId === null ? "completed" : "running",
          updatedAt: nowIso()
        };

        if (node.fanOut !== undefined) {
          return this.executeFanOut(nodeId, node.fanOut.parallelTo, node.fanOut.joinAt, nextState);
        }

        if (shouldInterruptAfter(this.interruptConfig, nodeId)) {
          return this.suspendRun(nextState, nodeId, "interrupt-after", "after");
        }

        return this.persistCheckpoint(nextState);
      } catch (error) {
        if (error instanceof DynamicInterrupt) {
          const patchedState =
            error.patch === undefined
              ? state
              : {
                  ...state,
                  channels: this.applyUpdate(state.channels as Record<string, unknown>, error.patch),
                  version: state.version + 1
                };
          return this.suspendRun(patchedState, nodeId, error.reason, "during");
        }
        const message = error instanceof Error ? error.message : "Unknown node error.";
        const category = classifyFailure(error);
        this.eventBus.emit({
          type: "node_failed",
          runId: state.runId,
          nodeId,
          error: message,
          attempt,
          category,
          timestamp: nowIso()
        });
        await this.callbackManager.emit({
          type: "onNodeError",
          runId: String(state.runId),
          nodeId: String(nodeId),
          timestamp: nowIso(),
          error: message
        });

        if (attempt >= maxAttempts) {
          // ADR 0076: retries are exhausted. A node with a declared `"error"` outgoing edge
          // reroutes there (a real, auditable branch) instead of failing the whole run — absent
          // one, behavior is UNCHANGED from before this feature existed (rethrow, run fails).
          const errorEdge = this.findErrorEdge(nodeId);
          if (errorEdge !== undefined) {
            const routedChannels = {
              ...state.channels,
              __lastError: { nodeId: String(nodeId), message, category, attempt }
            };
            const routedState: GraphState = {
              ...state,
              currentNodeId: errorEdge.to,
              channels: routedChannels,
              version: state.version + 1,
              status: "running",
              updatedAt: nowIso()
            };
            this.eventBus.emit({
              type: "node_error_routed",
              runId: state.runId,
              nodeId,
              errorEdgeId: errorEdge.id,
              toNodeId: errorEdge.to,
              category,
              error: message,
              timestamp: nowIso()
            });
            return this.persistCheckpoint(routedState);
          }
          throw error;
        }

        if (backoffMs > 0) {
          await new Promise((resolve) => setTimeout(resolve, backoffMs));
        }
      }
    }

    throw new Error("Node execution failed after retry attempts.");
  }

  public nextNode(currentNodeId: NodeId, state: GraphState): NodeId | null {
    const outgoingEdges = this.graph.edges.filter((edge) => edge.from === currentNodeId);
    if (outgoingEdges.length === 0) {
      return null;
    }

    const selectedEdge = this.selectNextEdge(outgoingEdges, state);
    return selectedEdge?.to ?? null;
  }

  public async send(runId: RunId, nodeId: NodeId, input: unknown): Promise<void> {
    const queueByNode = this.inboxByRunId.get(runId) ?? new Map<NodeId, unknown[]>();
    const queue = queueByNode.get(nodeId) ?? [];
    queueByNode.set(nodeId, [...queue, input]);
    this.inboxByRunId.set(runId, queueByNode);
  }

  /** ADR 0076 — at most one `"error"` edge per node is enforced at validation time
   * (`GRAPH_VALIDATION_ERROR_CODES.MULTIPLE_ERROR_EDGES`), so the first match is unambiguous. */
  private findErrorEdge(nodeId: NodeId): EdgeDefinition | undefined {
    return this.graph.edges.find((edge) => edge.from === nodeId && edge.type === "error");
  }

  private selectNextEdge(edges: EdgeDefinition[], state: GraphState): EdgeDefinition | undefined {
    const conditionalEdges = edges.filter((edge) => edge.type === "conditional");
    for (const edge of conditionalEdges) {
      if (edge.condition === undefined) {
        continue;
      }
      const conditionFn = this.conditionRegistry.resolve(edge.condition);
      if (conditionFn?.(state) === true) {
        return edge;
      }
    }

    return edges.find((edge) => edge.type === "default");
  }

  private async runLoop(state: GraphState, resumed: boolean): Promise<GraphState> {
    let currentState = state;
    while (currentState.status === "running") {
      try {
        currentState = await this.executeNode(currentState.currentNodeId, currentState);
      } catch (error) {
        const message = error instanceof Error ? error.message : "Unknown runtime error.";
        const failedState: GraphState = {
          ...currentState,
          status: "failed",
          updatedAt: nowIso()
        };
        currentState = await this.persistCheckpoint(failedState);
        this.eventBus.emit({
          type: "run_failed",
          runId: currentState.runId,
          error: message,
          timestamp: nowIso()
        });
        await this.callbackManager.emit({
          type: "onChainError",
          runId: String(currentState.runId),
          timestamp: nowIso(),
          error: message
        });
        return currentState;
      }
    }

    if (currentState.status === "completed") {
      this.eventBus.emit({
        type: "run_completed",
        runId: currentState.runId,
        finalState: currentState,
        timestamp: nowIso()
      });
      await this.callbackManager.emit({
        type: "onChainEnd",
        runId: String(currentState.runId),
        timestamp: nowIso(),
        output: currentState
      });
    }

    // resumed flag intentionally kept for future extension hooks.
    void resumed;

    return currentState;
  }

  private async persistCheckpoint(state: GraphState): Promise<GraphState> {
    const checkpoint = createCheckpoint(state.runId, state);
    await this.checkpointer.save(checkpoint);

    const persisted = {
      ...state,
      checkpointId: checkpoint.id,
      updatedAt: nowIso()
    };
    const history = this.stateHistoryByRunId.get(state.runId) ?? [];
    this.stateHistoryByRunId.set(state.runId, [...history, persisted]);
    return persisted;
  }

  public getHistory(runId: RunId): GraphState<ChannelsSchema>[] {
    return [...(this.stateHistoryByRunId.get(runId) ?? [])];
  }

  public async updateState(
    runId: RunId,
    patch: Partial<Record<string, unknown>>,
    resumeFrom?: NodeId
  ): Promise<GraphState> {
    const checkpoint = await this.checkpointer.load(runId);
    if (checkpoint === undefined) {
      throw new Error(`No checkpoint found for run '${String(runId)}'.`);
    }

    const nextChannels = this.applyUpdate(checkpoint.graphState.channels as Record<string, unknown>, patch);
    const nextState: GraphState = {
      ...checkpoint.graphState,
      channels: this.clearInterruptMeta(nextChannels),
      currentNodeId: (resumeFrom ?? checkpoint.graphState.currentNodeId) as NodeId,
      status: "running",
      version: checkpoint.graphState.version + 1,
      updatedAt: nowIso()
    };

    return this.persistCheckpoint(nextState);
  }

  public async getCheckpoints(runId: RunId): Promise<Checkpoint[]> {
    return this.checkpointer.list(runId);
  }

  public async replayFrom(runId: RunId, checkpointId: CheckpointId): Promise<GraphState> {
    const checkpoint = await this.checkpointer.loadById(checkpointId);
    if (checkpoint === undefined || checkpoint.runId !== runId) {
      throw new Error(`Checkpoint '${String(checkpointId)}' not found for run '${String(runId)}'.`);
    }

    const forkRunId = createForkRunId(runId);
    const forkedState: GraphState = {
      ...checkpoint.graphState,
      runId: forkRunId,
      status: "running",
      checkpointId: undefined,
      updatedAt: nowIso()
    };
    const persisted = await this.persistCheckpoint(forkedState);
    return this.runLoop(persisted, false);
  }

  public applyUpdate(
    channels: Record<string, unknown>,
    partialUpdate: Partial<Record<string, unknown>>
  ): Record<string, unknown> {
    const nextChannels = { ...channels };
    for (const [channelName, deltaValue] of Object.entries(partialUpdate)) {
      const definition = this.graph.channels[channelName] as ChannelDefinition<unknown> | undefined;
      const reducer = definition?.reducer ?? "replace";
      const currentValue = nextChannels[channelName];

      if (reducer === "replace") {
        nextChannels[channelName] = deltaValue;
        continue;
      }

      if (reducer === "append") {
        if (Array.isArray(currentValue) && Array.isArray(deltaValue)) {
          nextChannels[channelName] = [...currentValue, ...deltaValue];
        } else {
          nextChannels[channelName] = deltaValue;
        }
        continue;
      }

      if (
        reducer === "merge" &&
        currentValue !== null &&
        typeof currentValue === "object" &&
        deltaValue !== null &&
        typeof deltaValue === "object" &&
        !Array.isArray(currentValue) &&
        !Array.isArray(deltaValue)
      ) {
        nextChannels[channelName] = {
          ...(currentValue as Record<string, unknown>),
          ...(deltaValue as Record<string, unknown>)
        };
      } else {
        nextChannels[channelName] = deltaValue;
      }
    }

    return nextChannels;
  }

  private buildInitialChannels(initialData: Record<string, unknown>): Record<string, unknown> {
    const channels: Record<string, unknown> = {};
    for (const [name, definition] of Object.entries(this.graph.channels)) {
      channels[name] = name in initialData ? initialData[name] : definition.default;
    }
    for (const [name, value] of Object.entries(initialData)) {
      if (!(name in channels)) {
        channels[name] = value;
      }
    }
    return channels;
  }

  private applyInputMapping(
    parentData: Record<string, unknown>,
    inputMapping: Record<string, string> | undefined
  ): Record<string, unknown> {
    if (inputMapping === undefined) {
      return { ...parentData };
    }

    const mapped: Record<string, unknown> = {};
    for (const [childKey, parentKey] of Object.entries(inputMapping)) {
      mapped[childKey] = parentData[parentKey];
    }
    return mapped;
  }

  private applyOutputMapping(
    parentData: Record<string, unknown>,
    childData: Record<string, unknown>,
    outputMapping: Record<string, string> | undefined
  ): Record<string, unknown> {
    if (outputMapping === undefined) {
      return { ...parentData, ...childData };
    }

    const merged = { ...parentData };
    for (const [parentKey, childKey] of Object.entries(outputMapping)) {
      merged[parentKey] = childData[childKey];
    }
    return merged;
  }

  private setSubgraphRunId(
    data: Record<string, unknown>,
    nodeId: NodeId,
    childRunId: RunId
  ): Record<string, unknown> {
    const currentMap =
      data[SUBGRAPH_RUNS_KEY] !== null && typeof data[SUBGRAPH_RUNS_KEY] === "object"
        ? (data[SUBGRAPH_RUNS_KEY] as Record<string, string>)
        : {};

    return {
      ...data,
      [SUBGRAPH_RUNS_KEY]: {
        ...currentMap,
        [String(nodeId)]: String(childRunId)
      }
    };
  }

  private getOrCreateSubgraphRunId(state: GraphState, nodeId: NodeId): RunId {
    const maybeMap = (state.channels as Record<string, unknown>)[SUBGRAPH_RUNS_KEY];
    if (maybeMap !== null && typeof maybeMap === "object") {
      const existing = (maybeMap as Record<string, string>)[String(nodeId)];
      if (typeof existing === "string") {
        return existing as RunId;
      }
    }
    return `${String(state.runId)}:${String(nodeId)}` as RunId;
  }

  private readInterruptMeta(channels: Record<string, unknown>): { when: "before" | "after" | "during" } | undefined {
    const raw = channels[INTERRUPT_META_KEY];
    if (raw !== null && typeof raw === "object") {
      const when = (raw as Record<string, unknown>).when;
      if (when === "before" || when === "after" || when === "during") {
        return { when };
      }
    }
    return undefined;
  }

  private clearInterruptMeta(channels: Record<string, unknown>): Record<string, unknown> {
    const next = { ...channels };
    delete next[INTERRUPT_META_KEY];
    return next;
  }

  private async suspendRun(
    state: GraphState,
    nodeId: NodeId,
    reason: string,
    when: "before" | "after" | "during"
  ): Promise<GraphState> {
    const suspendedState: GraphState = {
      ...state,
      currentNodeId: nodeId,
      status: "suspended",
      channels: {
        ...(state.channels as Record<string, unknown>),
        [INTERRUPT_META_KEY]: { when, reason }
      },
      updatedAt: nowIso()
    };
    const persisted = await this.persistCheckpoint(suspendedState);
    this.eventBus.emit({
      type: "run_suspended",
      runId: state.runId,
      nodeId,
      reason,
      timestamp: nowIso()
    });
    return persisted;
  }

  private computeDelta(
    previousChannels: Record<string, unknown>,
    nextChannels: Record<string, unknown>
  ): Record<string, unknown> {
    const delta: Record<string, unknown> = {};
    const keys = new Set<string>([...Object.keys(previousChannels), ...Object.keys(nextChannels)]);
    for (const key of keys) {
      if (!this.areEqual(previousChannels[key], nextChannels[key])) {
        delta[key] = nextChannels[key];
      }
    }
    return delta;
  }

  private areEqual(left: unknown, right: unknown): boolean {
    return structuralEqual(left, right);
  }

  private assertRecursionLimit(runId: RunId): void {
    const current = this.stepsByRunId.get(runId) ?? 0;
    const next = current + 1;
    this.stepsByRunId.set(runId, next);
    const recursionLimit = this.graph.recursionLimit ?? 25;
    if (next > recursionLimit) {
      throw new RecursionLimitError(recursionLimit);
    }
  }

  private consumeStepBudget(): void {
    this.stepBudget.currentSteps += 1;
    checkBudget(this.stepBudget);
  }

  private consumeInjectedInput(runId: RunId, nodeId: NodeId): unknown | undefined {
    const queueByNode = this.inboxByRunId.get(runId);
    if (queueByNode === undefined) {
      return undefined;
    }
    const queue = queueByNode.get(nodeId);
    if (queue === undefined || queue.length === 0) {
      return undefined;
    }
    const [first, ...rest] = queue;
    queueByNode.set(nodeId, rest);
    return first;
  }

  private resolveCommand(
    output: Partial<Record<string, unknown>> | Command
  ): { goto?: NodeId | NodeId[]; update: Partial<Record<string, unknown>> } {
    if (isSwarmHandoff(output)) {
      return {
        goto: output.goto as unknown as NodeId,
        update: output.update as Partial<Record<string, unknown>>
      };
    }
    if (output !== null && typeof output === "object" && "goto" in output) {
      const cmd = output as Command;
      return {
        goto: cmd.goto,
        update: (cmd.update ?? {}) as Partial<Record<string, unknown>>
      };
    }
    return {
      update: (output ?? {}) as Partial<Record<string, unknown>>
    };
  }

  private resolveNextNode(
    currentNodeId: NodeId,
    goto: NodeId | NodeId[] | undefined,
    state: GraphState
  ): NodeId | null {
    if (goto === undefined) {
      return this.nextNode(currentNodeId, state);
    }
    if (Array.isArray(goto)) {
      return goto[0] ?? null;
    }
    return goto;
  }

  private async executeFanOut(
    fromNodeId: NodeId,
    parallelTo: NodeId[],
    joinAt: NodeId,
    baseState: GraphState
  ): Promise<GraphState> {
    const runId = baseState.runId;
    const nodeStates = await Promise.all(
      parallelTo.map(async (nodeId) => {
        const handler = this.nodeRegistry.resolve(nodeId);
        if (handler === undefined) {
          return {};
        }
        const output = await handler(baseState.channels, baseState, { memory: this.memory });
        const { update } = this.resolveCommand(output as Partial<Record<string, unknown>> | Command);
        return update;
      })
    );
    let channels = baseState.channels as Record<string, unknown>;
    for (const update of nodeStates) {
      channels = this.applyUpdate(channels, update);
    }
    const nextState: GraphState = {
      ...baseState,
      runId,
      currentNodeId: joinAt,
      channels,
      version: baseState.version + 1,
      status: "running",
      updatedAt: nowIso()
    };
    this.eventBus.emit({
      type: "node_completed",
      runId,
      nodeId: fromNodeId,
      output: nodeStates,
      timestamp: nowIso()
    });
    return this.persistCheckpoint(nextState);
  }

  private extractMessageEvents(nodeId: NodeId, delta: Record<string, unknown>): StreamEvent[] {
    const events: StreamEvent[] = [];
    for (const value of Object.values(delta)) {
      if (value !== null && typeof value === "object" && !Array.isArray(value)) {
        const record = value as Record<string, unknown>;
        const kind = typeof record.type === "string" ? record.type.toLowerCase() : "";
        const messageText =
          typeof record.delta === "string"
            ? record.delta
            : typeof record.content === "string"
              ? record.content
              : undefined;
        if ((kind === "aimessage" || kind === "humanmessage" || kind === "toolmessage") && messageText) {
          events.push({
            type: "message_delta",
            delta: messageText,
            nodeId,
            messageId:
              typeof record.messageId === "string"
                ? record.messageId
                : `${String(nodeId)}:${Math.random().toString(36).slice(2, 8)}`
          });
        }
      }
    }
    return events;
  }

  private extractToolCallEvents(nodeId: NodeId, delta: Record<string, unknown>): StreamEvent[] {
    const events: StreamEvent[] = [];
    for (const value of Object.values(delta)) {
      if (value !== null && typeof value === "object" && !Array.isArray(value)) {
        const record = value as Record<string, unknown>;
        if (typeof record.toolId === "string") {
          events.push({
            type: "tool_call",
            nodeId,
            toolId: record.toolId,
            input: record.input
          });
        }
      }
    }
    return events;
  }
}
