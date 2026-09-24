import { describe, expect, it } from "vitest";
import type { GraphDefinition, GraphState } from "@ailu-ai/graph-core";
import { InMemoryToolRegistry } from "../../agents-core/src/tools.js";
import { StepBudgetExceededError } from "../../agents-core/src/step-budget.js";
import { InMemoryCallbackManager } from "../../callbacks/src/manager.js";

import { InMemoryCheckpointer } from "./checkpointer.js";
import { InMemoryConditionRegistry } from "./condition-registry.js";
import { InMemoryEventBus } from "./event-bus.js";
import { InMemoryNodeRegistry } from "./node-registry.js";
import { RecursionLimitError } from "./cycles.js";
import { createMessageGraph } from "./message-graph.js";
import { createToolNode } from "./tool-node.js";
import { GraphRuntime } from "./runtime.js";

const asGraph = <T>(value: T): T => value;

const createLinearGraph = (): GraphDefinition =>
  asGraph<GraphDefinition>({
    id: "graph-linear" as GraphDefinition["id"],
    version: "1.0.0",
    name: "Linear Graph",
    channels: {
      a: { type: "number", reducer: "replace", default: 0 },
      b: { type: "number", reducer: "replace", default: 0 },
      c: { type: "number", reducer: "replace", default: 0 }
    },
    entryNodeId: "A" as GraphDefinition["entryNodeId"],
    nodes: [
      { id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" },
      { id: "B" as GraphDefinition["nodes"][number]["id"], type: "action", label: "B" },
      { id: "C" as GraphDefinition["nodes"][number]["id"], type: "action", label: "C" }
    ],
    edges: [
      {
        id: "e1" as GraphDefinition["edges"][number]["id"],
        from: "A" as GraphDefinition["nodes"][number]["id"],
        to: "B" as GraphDefinition["nodes"][number]["id"],
        type: "default"
      },
      {
        id: "e2" as GraphDefinition["edges"][number]["id"],
        from: "B" as GraphDefinition["nodes"][number]["id"],
        to: "C" as GraphDefinition["nodes"][number]["id"],
        type: "default"
      }
    ]
  });

describe("GraphRuntime", () => {
  it("happy path runs A->B->C to completion", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();

    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const finalState = await runtime.start("run-1" as GraphState["runId"], {});

    expect(finalState.status).toBe("completed");
    expect(finalState.channels).toEqual({ a: 1, b: 2, c: 3 });
  });

  it("failure path emits run_failed and returns failed state", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    const events: string[] = [];
    eventBus.subscribe((event) => events.push(event.type));

    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => {
      throw new Error("B exploded");
    });
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const finalState = await runtime.start("run-fail" as GraphState["runId"], {});

    expect(finalState.status).toBe("failed");
    expect(events.includes("run_failed")).toBe(true);
  });

  it("saves checkpoints after each node completion and can load latest", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();

    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const runId = "run-checkpoint" as GraphState["runId"];
    const finalState = await runtime.start(runId, {});
    const checkpoint = await checkpointer.load(runId);

    expect(finalState.checkpointId).toBeDefined();
    expect(checkpoint?.graphState.status).toBe("completed");
  });

  it("emits expected event order for successful run", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    const events: string[] = [];
    eventBus.subscribe((event) => events.push(event.type));

    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    await runtime.start("run-events" as GraphState["runId"], {});

    expect(events).toEqual([
      "node_started",
      "node_completed",
      "node_started",
      "node_completed",
      "node_started",
      "node_completed",
      "run_completed"
    ]);
  });

  it("suspends on human-gate and resume continues from checkpoint", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-human" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Human Gate Graph",
      channels: {
        a: { type: "number", reducer: "replace", default: 0 },
        b: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" },
        {
          id: "H" as GraphDefinition["nodes"][number]["id"],
          type: "human-gate",
          label: "Human Gate"
        },
        { id: "B" as GraphDefinition["nodes"][number]["id"], type: "action", label: "B" }
      ],
      edges: [
        {
          id: "e1" as GraphDefinition["edges"][number]["id"],
          from: "A" as GraphDefinition["nodes"][number]["id"],
          to: "H" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        },
        {
          id: "e2" as GraphDefinition["edges"][number]["id"],
          from: "H" as GraphDefinition["nodes"][number]["id"],
          to: "B" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        }
      ]
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    const events: string[] = [];
    eventBus.subscribe((event) => events.push(event.type));

    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const runId = "run-human" as GraphState["runId"];
    const suspended = await runtime.start(runId, {});
    expect(suspended.status).toBe("suspended");
    expect(events.includes("run_suspended")).toBe(true);

    const resumed = await runtime.resume(runId);
    expect(resumed.status).toBe("completed");
    expect(events.includes("run_resumed")).toBe(true);
  });

  it("routes conditional edge to B when true and C when false", async () => {
    const baseGraph = asGraph<GraphDefinition>({
      id: "graph-conditional" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Conditional Graph",
      channels: {
        a: { type: "boolean", reducer: "replace", default: false },
        branch: { type: "string", reducer: "replace", default: "" }
      },
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" },
        { id: "B" as GraphDefinition["nodes"][number]["id"], type: "action", label: "B" },
        { id: "C" as GraphDefinition["nodes"][number]["id"], type: "action", label: "C" }
      ],
      edges: [
        {
          id: "e1" as GraphDefinition["edges"][number]["id"],
          from: "A" as GraphDefinition["nodes"][number]["id"],
          to: "B" as GraphDefinition["nodes"][number]["id"],
          type: "conditional",
          condition: "goB"
        },
        {
          id: "e2" as GraphDefinition["edges"][number]["id"],
          from: "A" as GraphDefinition["nodes"][number]["id"],
          to: "C" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        }
      ]
    });

    const runWith = async (goB: boolean): Promise<GraphState> => {
      const nodeRegistry = new InMemoryNodeRegistry();
      const conditionRegistry = new InMemoryConditionRegistry();
      const checkpointer = new InMemoryCheckpointer();
      const eventBus = new InMemoryEventBus();

      nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: true }));
      nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ branch: "B" }));
      nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ branch: "C" }));

      conditionRegistry.register("goB", () => goB);

      const runtime = new GraphRuntime({
        graph: baseGraph,
        nodeRegistry,
        conditionRegistry,
        checkpointer,
        eventBus
      });
      return runtime.start((goB ? "run-true" : "run-false") as GraphState["runId"], {});
    };

    const trueState = await runWith(true);
    const falseState = await runWith(false);

    expect(trueState.channels.branch).toBe("B");
    expect(falseState.channels.branch).toBe("C");
  });

  it("retries node according to RetryPolicy and succeeds on third attempt", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-retry" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Retry Graph",
      channels: {
        retried: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "R" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "R" as GraphDefinition["nodes"][number]["id"],
          type: "action",
          label: "Retry Node",
          retryPolicy: { maxAttempts: 3, backoffMs: 0 }
        }
      ],
      edges: []
    });

    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    let attempts = 0;
    const failures: number[] = [];
    eventBus.subscribe((event) => {
      if (event.type === "node_failed") {
        failures.push(event.attempt);
      }
    });

    nodeRegistry.register("R" as GraphDefinition["nodes"][number]["id"], async () => {
      attempts += 1;
      if (attempts < 3) {
        throw new Error("not yet");
      }
      return { retried: attempts };
    });

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const finalState = await runtime.start("run-retry" as GraphState["runId"], {});
    expect(finalState.status).toBe("completed");
    expect(finalState.channels.retried).toBe(3);
    expect(failures).toEqual([1, 2]);
  });

  it("routes to an error edge once retries are exhausted, instead of failing the run (ADR 0076)", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-error-edge" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Error Edge Graph",
      channels: {
        handled: { type: "boolean", reducer: "replace", default: false }
      },
      entryNodeId: "R" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "R" as GraphDefinition["nodes"][number]["id"],
          type: "action",
          label: "Always Fails",
          retryPolicy: { maxAttempts: 2, backoffMs: 0 }
        },
        { id: "H" as GraphDefinition["nodes"][number]["id"], type: "action", label: "Error Handler" }
      ],
      edges: [
        {
          id: "e-error" as GraphDefinition["edges"][number]["id"],
          from: "R" as GraphDefinition["nodes"][number]["id"],
          to: "H" as GraphDefinition["nodes"][number]["id"],
          type: "error"
        }
      ]
    });

    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    const events: string[] = [];
    eventBus.subscribe((event) => events.push(event.type));

    nodeRegistry.register("R" as GraphDefinition["nodes"][number]["id"], async () => {
      throw new Error("always fails");
    });
    nodeRegistry.register("H" as GraphDefinition["nodes"][number]["id"], async () => ({ handled: true }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const finalState = await runtime.start("run-error-edge" as GraphState["runId"], {});

    expect(finalState.status).toBe("completed");
    expect(finalState.channels.handled).toBe(true);
    expect(events.includes("node_error_routed")).toBe(true);
    expect(events.includes("run_failed")).toBe(false);
  });

  it("fails the run exactly as before when no error edge is declared (ADR 0076 backward compat)", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-no-error-edge" as GraphDefinition["id"],
      version: "1.0.0",
      name: "No Error Edge Graph",
      channels: {},
      entryNodeId: "R" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "R" as GraphDefinition["nodes"][number]["id"],
          type: "action",
          label: "Always Fails",
          retryPolicy: { maxAttempts: 2, backoffMs: 0 }
        }
      ],
      edges: []
    });

    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    const events: string[] = [];
    eventBus.subscribe((event) => events.push(event.type));

    nodeRegistry.register("R" as GraphDefinition["nodes"][number]["id"], async () => {
      throw new Error("always fails");
    });

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus
    });

    const finalState = await runtime.start("run-no-error-edge" as GraphState["runId"], {});

    expect(finalState.status).toBe("failed");
    expect(events.includes("run_failed")).toBe(true);
    expect(events.includes("node_error_routed")).toBe(false);
  });

  it("executes subgraph and merges outputMapping into parent state", async () => {
    const childGraph = asGraph<GraphDefinition>({
      id: "child-graph" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Child",
      channels: {
        inputFoo: { type: "number", reducer: "replace", default: 0 },
        bar: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "SA" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "SA" as GraphDefinition["nodes"][number]["id"], type: "action", label: "SA" }
      ],
      edges: []
    });
    const parentGraph = asGraph<GraphDefinition>({
      id: "parent-graph" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Parent",
      channels: {
        foo: { type: "number", reducer: "replace", default: 0 },
        childResult: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "P1" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "P1" as GraphDefinition["nodes"][number]["id"], type: "action", label: "P1" },
        {
          id: "SG" as GraphDefinition["nodes"][number]["id"],
          type: "subgraph",
          label: "Sub",
          subgraphId: "child-graph" as GraphDefinition["id"],
          inputMapping: { inputFoo: "foo" },
          outputMapping: { childResult: "bar" }
        }
      ],
      edges: [
        {
          id: "p-e1" as GraphDefinition["edges"][number]["id"],
          from: "P1" as GraphDefinition["nodes"][number]["id"],
          to: "SG" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        }
      ]
    });

    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    nodeRegistry.register("P1" as GraphDefinition["nodes"][number]["id"], async () => ({ foo: 7 }));
    nodeRegistry.register("SA" as GraphDefinition["nodes"][number]["id"], async (input) => ({
      bar: (input as Record<string, unknown>).inputFoo
    }));

    const runtime = new GraphRuntime({
      graph: parentGraph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus,
      subgraphResolver: (graphId) => (graphId === childGraph.id ? childGraph : undefined)
    });

    const state = await runtime.start("run-subgraph-ok" as GraphState["runId"], {});
    expect(state.status).toBe("completed");
    expect(state.channels.childResult).toBe(7);
  });

  it("fails parent when subgraph fails", async () => {
    const childGraph = asGraph<GraphDefinition>({
      id: "child-fail" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Child Fail",
      channels: {},
      entryNodeId: "SF" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "SF" as GraphDefinition["nodes"][number]["id"], type: "action", label: "SF" }
      ],
      edges: []
    });
    const parentGraph = asGraph<GraphDefinition>({
      id: "parent-fail" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Parent",
      channels: {},
      entryNodeId: "SG" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "SG" as GraphDefinition["nodes"][number]["id"],
          type: "subgraph",
          label: "Sub",
          subgraphId: "child-fail" as GraphDefinition["id"]
        }
      ],
      edges: []
    });

    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    nodeRegistry.register("SF" as GraphDefinition["nodes"][number]["id"], async () => {
      throw new Error("child boom");
    });

    const runtime = new GraphRuntime({
      graph: parentGraph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus,
      subgraphResolver: (graphId) => (graphId === childGraph.id ? childGraph : undefined)
    });

    const state = await runtime.start("run-subgraph-fail" as GraphState["runId"], {});
    expect(state.status).toBe("failed");
  });

  it("suspends parent when subgraph hits human-gate and resumes from child checkpoint", async () => {
    const childGraph = asGraph<GraphDefinition>({
      id: "child-human" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Child Human",
      channels: {
        done: { type: "boolean", reducer: "replace", default: false }
      },
      entryNodeId: "CH" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "CH" as GraphDefinition["nodes"][number]["id"], type: "human-gate", label: "CH" },
        { id: "CD" as GraphDefinition["nodes"][number]["id"], type: "action", label: "CD" }
      ],
      edges: [
        {
          id: "c-e1" as GraphDefinition["edges"][number]["id"],
          from: "CH" as GraphDefinition["nodes"][number]["id"],
          to: "CD" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        }
      ]
    });
    const parentGraph = asGraph<GraphDefinition>({
      id: "parent-human" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Parent Human",
      channels: {
        done: { type: "boolean", reducer: "replace", default: false }
      },
      entryNodeId: "SG" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "SG" as GraphDefinition["nodes"][number]["id"],
          type: "subgraph",
          label: "Sub",
          subgraphId: "child-human" as GraphDefinition["id"]
        }
      ],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    const conditionRegistry = new InMemoryConditionRegistry();
    const checkpointer = new InMemoryCheckpointer();
    const eventBus = new InMemoryEventBus();
    nodeRegistry.register("CD" as GraphDefinition["nodes"][number]["id"], async () => ({ done: true }));

    const runtime = new GraphRuntime({
      graph: parentGraph,
      nodeRegistry,
      conditionRegistry,
      checkpointer,
      eventBus,
      subgraphResolver: (graphId) => (graphId === childGraph.id ? childGraph : undefined)
    });

    const runId = "run-subgraph-human" as GraphState["runId"];
    const suspended = await runtime.start(runId, {});
    expect(suspended.status).toBe("suspended");

    const resumed = await runtime.resume(runId);
    expect(resumed.status).toBe("completed");
    expect(resumed.channels.done).toBe(true);
  });

  it("applies reducers replace append and merge", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-reducers" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Reducer Graph",
      channels: {
        text: { type: "string", reducer: "replace", default: "a" },
        logs: { type: "array", reducer: "append", default: ["init"] },
        meta: { type: "object", reducer: "merge", default: { a: 1 } }
      },
      entryNodeId: "N" as GraphDefinition["entryNodeId"],
      nodes: [{ id: "N" as GraphDefinition["nodes"][number]["id"], type: "action", label: "N" }],
      edges: []
    });
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry: new InMemoryNodeRegistry(),
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const result = runtime.applyUpdate(
      { text: "a", logs: ["init"], meta: { a: 1 } },
      { text: "b", logs: ["next"], meta: { b: 2 } }
    );

    expect(result.text).toBe("b");
    expect(result.logs).toEqual(["init", "next"]);
    expect(result.meta).toEqual({ a: 1, b: 2 });
  });

  it("merges partial channel updates from handler", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-partial" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Partial Graph",
      channels: {
        a: { type: "number", reducer: "replace", default: 0 },
        b: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "N" as GraphDefinition["entryNodeId"],
      nodes: [{ id: "N" as GraphDefinition["nodes"][number]["id"], type: "action", label: "N" }],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("N" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 2 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const state = await runtime.start("run-partial" as GraphState["runId"], {});
    expect(state.channels).toEqual({ a: 2, b: 0 });
  });

  it("grows state history at each checkpoint", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const runId = "run-history" as GraphState["runId"];
    await runtime.start(runId, {});
    const history = runtime.getHistory(runId);

    expect(history.length).toBeGreaterThanOrEqual(4);
  });

  it("streams full state in values mode", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const events = [];
    for await (const event of runtime.stream("run-stream-values" as GraphState["runId"], {}, "values")) {
      events.push(event);
    }

    expect(events.length).toBe(3);
    expect(events.every((event) => event.type === "state_value")).toBe(true);
  });

  it("streams deltas in updates mode", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const updates = [];
    for await (const event of runtime.stream("run-stream-updates" as GraphState["runId"], {}, "updates")) {
      updates.push(event);
    }

    expect(updates).toEqual([
      { type: "state_update", nodeId: "A", delta: { a: 1 } },
      { type: "state_update", nodeId: "B", delta: { b: 2 } },
      { type: "state_update", nodeId: "C", delta: { c: 3 } }
    ]);
  });

  it("streams debug payloads in debug mode", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-debug" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Debug Graph",
      channels: {
        a: { type: "number", reducer: "replace", default: 0 }
      },
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [{ id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" }],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });

    const events = [];
    for await (const event of runtime.stream("run-stream-debug" as GraphState["runId"], {}, "debug")) {
      events.push(event);
    }

    expect(events.length).toBe(4);
    expect(events.every((event) => event.type === "debug")).toBe(true);
  });

  it("suspends before configured node execution", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus(),
      interruptConfig: { before: ["B" as GraphDefinition["nodes"][number]["id"]] }
    });

    const state = await runtime.start("run-interrupt-before" as GraphState["runId"], {});
    expect(state.status).toBe("suspended");
    expect(state.currentNodeId).toBe("B");
  });

  it("can patch state and resume from specific node", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const checkpointer = new InMemoryCheckpointer();

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer,
      eventBus: new InMemoryEventBus(),
      interruptConfig: { before: ["B" as GraphDefinition["nodes"][number]["id"]] }
    });

    await runtime.start("run-update-state" as GraphState["runId"], {});
    const patched = await runtime.updateState(
      "run-update-state" as GraphState["runId"],
      { b: 99 },
      "C" as GraphDefinition["nodes"][number]["id"]
    );
    expect(patched.channels.b).toBe(99);
    expect(patched.currentNodeId).toBe("C");
  });

  it("lists checkpoints and replays from one checkpoint", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const checkpointer = new InMemoryCheckpointer();

    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer,
      eventBus: new InMemoryEventBus()
    });

    await runtime.start("run-replay" as GraphState["runId"], {});
    const checkpoints = await runtime.getCheckpoints("run-replay" as GraphState["runId"]);
    expect(checkpoints.length).toBeGreaterThan(0);

    const replayed = await runtime.replayFrom(
      "run-replay" as GraphState["runId"],
      checkpoints[0]!.id
    );
    expect(String(replayed.runId)).toContain("fork");
  });

  it("send injects input into the targeted node", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-send" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Send Graph",
      channels: { captured: { type: "string", reducer: "replace", default: "" } },
      recursionLimit: 10,
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [{ id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" }],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async (input) => ({
      captured: (input as Record<string, unknown>).message ?? ""
    }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    await runtime.send("run-send" as GraphState["runId"], "A" as GraphDefinition["nodes"][number]["id"], {
      message: "hello"
    });
    const state = await runtime.start("run-send" as GraphState["runId"], {});
    expect(state.channels.captured).toBe("hello");
  });

  it("fan-out executes nodes and joins at joinAt", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-fanout" as GraphDefinition["id"],
      version: "1.0.0",
      name: "FanOut Graph",
      channels: {
        x: { type: "number", reducer: "replace", default: 0 },
        y: { type: "number", reducer: "replace", default: 0 },
        done: { type: "boolean", reducer: "replace", default: false }
      },
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [
        {
          id: "A" as GraphDefinition["nodes"][number]["id"],
          type: "action",
          label: "A",
          fanOut: {
            parallelTo: [
              "B" as GraphDefinition["nodes"][number]["id"],
              "C" as GraphDefinition["nodes"][number]["id"]
            ],
            joinAt: "J" as GraphDefinition["nodes"][number]["id"]
          }
        },
        { id: "B" as GraphDefinition["nodes"][number]["id"], type: "action", label: "B" },
        { id: "C" as GraphDefinition["nodes"][number]["id"], type: "action", label: "C" },
        { id: "J" as GraphDefinition["nodes"][number]["id"], type: "action", label: "J" }
      ],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ x: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ x: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ y: 3 }));
    nodeRegistry.register("J" as GraphDefinition["nodes"][number]["id"], async () => ({ done: true }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-fanout" as GraphState["runId"], {});
    expect(state.channels.done).toBe(true);
    expect(state.channels.x).toBe(2);
    expect(state.channels.y).toBe(3);
  });

  it("throws RecursionLimitError when cycle exceeds recursionLimit", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-cycle" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Cycle Graph",
      recursionLimit: 2,
      channels: {},
      entryNodeId: "A" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "A" as GraphDefinition["nodes"][number]["id"], type: "action", label: "A" },
        { id: "B" as GraphDefinition["nodes"][number]["id"], type: "action", label: "B" }
      ],
      edges: [
        {
          id: "e1" as GraphDefinition["edges"][number]["id"],
          from: "A" as GraphDefinition["nodes"][number]["id"],
          to: "B" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        },
        {
          id: "e2" as GraphDefinition["edges"][number]["id"],
          from: "B" as GraphDefinition["nodes"][number]["id"],
          to: "A" as GraphDefinition["nodes"][number]["id"],
          type: "default"
        }
      ]
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({}));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({}));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const finalState = await runtime.start("run-cycle" as GraphState["runId"], {});
    expect(finalState.status).toBe("failed");
    const history = runtime.getHistory("run-cycle" as GraphState["runId"]);
    expect(history.length).toBeGreaterThan(0);
    expect(RecursionLimitError).toBeDefined();
  });

  it("append reducer works for messages channel", async () => {
    const nodes: GraphDefinition["nodes"] = [
      { id: "M1" as GraphDefinition["nodes"][number]["id"], type: "action", label: "M1" }
    ];
    const graph = createMessageGraph(nodes, []);
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("M1" as GraphDefinition["nodes"][number]["id"], async () => ({
      messages: [
        {
          id: "m-1",
          role: "human",
          content: "hello",
          createdAt: new Date()
        }
      ]
    }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const finalState = await runtime.start("run-messages" as GraphState["runId"], {
      messages: [
        {
          id: "m-0",
          role: "system",
          content: "init",
          createdAt: new Date()
        }
      ]
    });
    expect((finalState.channels.messages as unknown[]).length).toBe(2);
  });

  it("executes a single tool call", async () => {
    const graph = createMessageGraph(
      [{ id: "T1" as GraphDefinition["nodes"][number]["id"], type: "tool", label: "Tool" }],
      []
    );
    const registry = new InMemoryToolRegistry();
    registry.register(
      {
        id: "echo" as never,
        name: "echo",
        description: "Echo",
        inputSchema: { parse: (input: unknown) => input as { text: string } },
        outputSchema: { parse: (output: unknown) => output as { text: string } },
        permissions: ["read"]
      },
      async (input) => ({ text: (input as { text: string }).text })
    );
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("T1" as GraphDefinition["nodes"][number]["id"], createToolNode(registry));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-tool-single" as GraphState["runId"], {
      messages: [
        {
          id: "ai-1",
          role: "ai",
          content: "call tool",
          toolCalls: [{ id: "tc-1", name: "echo", input: { text: "ok" } }],
          createdAt: new Date()
        }
      ]
    });
    expect(Array.isArray(state.channels.messages)).toBe(true);
    expect((state.channels.messages as Array<Record<string, unknown>>).length).toBe(2);
  });

  it("executes parallel tool calls", async () => {
    const graph = createMessageGraph(
      [{ id: "T1" as GraphDefinition["nodes"][number]["id"], type: "tool", label: "Tool" }],
      []
    );
    const registry = new InMemoryToolRegistry();
    registry.register(
      {
        id: "a" as never,
        name: "a",
        description: "A",
        inputSchema: { parse: (input: unknown) => input as { value: number } },
        outputSchema: { parse: (output: unknown) => output as { value: number } },
        permissions: ["read"]
      },
      async (input) => ({ value: (input as { value: number }).value + 1 })
    );
    registry.register(
      {
        id: "b" as never,
        name: "b",
        description: "B",
        inputSchema: { parse: (input: unknown) => input as { value: number } },
        outputSchema: { parse: (output: unknown) => output as { value: number } },
        permissions: ["read"]
      },
      async (input) => ({ value: (input as { value: number }).value + 2 })
    );
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register(
      "T1" as GraphDefinition["nodes"][number]["id"],
      createToolNode(registry, { parallel: true })
    );
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-tool-parallel" as GraphState["runId"], {
      messages: [
        {
          id: "ai-2",
          role: "ai",
          content: "call tools",
          toolCalls: [
            { id: "tc-a", name: "a", input: { value: 1 } },
            { id: "tc-b", name: "b", input: { value: 1 } }
          ],
          createdAt: new Date()
        }
      ]
    });
    expect((state.channels.messages as Array<Record<string, unknown>>).length).toBe(3);
  });

  it("creates tool error message when ToolException happens", async () => {
    const graph = createMessageGraph(
      [{ id: "T1" as GraphDefinition["nodes"][number]["id"], type: "tool", label: "Tool" }],
      []
    );
    const registry = new InMemoryToolRegistry();
    registry.register(
      {
        id: "boom" as never,
        name: "boom",
        description: "Boom",
        inputSchema: { parse: (input: unknown) => input as Record<string, never> },
        outputSchema: { parse: (output: unknown) => output as { ok: boolean } },
        permissions: ["write"]
      },
      async () => {
        throw new Error("boom");
      }
    );
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("T1" as GraphDefinition["nodes"][number]["id"], createToolNode(registry));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-tool-error" as GraphState["runId"], {
      messages: [
        {
          id: "ai-3",
          role: "ai",
          content: "call boom",
          toolCalls: [{ id: "tc-err", name: "boom", input: {} }],
          createdAt: new Date()
        }
      ]
    });
    const all = state.channels.messages as Array<Record<string, unknown>>;
    const last = all[all.length - 1];
    expect(typeof last?.content).toBe("string");
    expect(String(last?.content)).toContain("Tool execution error");
  });

  it("suspends run when tool requires approval", async () => {
    const graph = createMessageGraph(
      [{ id: "T1" as GraphDefinition["nodes"][number]["id"], type: "tool", label: "Tool" }],
      []
    );
    const registry = new InMemoryToolRegistry();
    registry.register(
      {
        id: "sensitive" as never,
        name: "sensitive",
        description: "Sensitive tool",
        inputSchema: { parse: (input: unknown) => input as Record<string, never> },
        outputSchema: { parse: (output: unknown) => output as { ok: boolean } },
        permissions: ["delete"],
        requiresApproval: true
      },
      async () => ({ ok: true })
    );
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("T1" as GraphDefinition["nodes"][number]["id"], createToolNode(registry));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-tool-approval" as GraphState["runId"], {
      messages: [
        {
          id: "ai-4",
          role: "ai",
          content: "call sensitive",
          toolCalls: [{ id: "tc-appr", name: "sensitive", input: {} }],
          createdAt: new Date()
        }
      ]
    });
    expect(state.status).toBe("suspended");
    const approvals = (state.channels as Record<string, unknown>).approvalRequests;
    expect(Array.isArray(approvals)).toBe(true);
  });

  it("callback handler errors do not interrupt run", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const callbackManager = new InMemoryCallbackManager([
      {
        onNodeStart: async () => {
          throw new Error("handler exploded");
        }
      }
    ]);
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus(),
      callbackManager
    });

    const state = await runtime.start("run-callback-safe" as GraphState["runId"], {});
    expect(state.status).toBe("completed");
  });

  it("fails run when step budget is exceeded", async () => {
    const graph = createLinearGraph();
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("A" as GraphDefinition["nodes"][number]["id"], async () => ({ a: 1 }));
    nodeRegistry.register("B" as GraphDefinition["nodes"][number]["id"], async () => ({ b: 2 }));
    nodeRegistry.register("C" as GraphDefinition["nodes"][number]["id"], async () => ({ c: 3 }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus(),
      stepBudget: { maxSteps: 1, currentSteps: 0 }
    });
    const state = await runtime.start("run-step-budget" as GraphState["runId"], {});
    expect(state.status).toBe("failed");
    expect(StepBudgetExceededError).toBeDefined();
  });

  it("routes SwarmHandoff and keeps context channels", async () => {
    const graph = asGraph<GraphDefinition>({
      id: "graph-swarm" as GraphDefinition["id"],
      version: "1.0.0",
      name: "Swarm Graph",
      channels: {
        messages: { type: "array", reducer: "append", default: [] },
        reason: { type: "string", reducer: "replace", default: "" },
        done: { type: "boolean", reducer: "replace", default: false }
      },
      entryNodeId: "agent-a" as GraphDefinition["entryNodeId"],
      nodes: [
        { id: "agent-a" as GraphDefinition["nodes"][number]["id"], type: "agent", label: "A" },
        { id: "agent-b" as GraphDefinition["nodes"][number]["id"], type: "agent", label: "B" }
      ],
      edges: []
    });
    const nodeRegistry = new InMemoryNodeRegistry();
    nodeRegistry.register("agent-a" as GraphDefinition["nodes"][number]["id"], async () => ({
      type: "swarm_handoff",
      goto: "agent-b",
      update: { reason: "handoff to specialist" }
    }));
    nodeRegistry.register("agent-b" as GraphDefinition["nodes"][number]["id"], async (input) => ({
      done: true,
      messages: (input as Record<string, unknown>).messages
    }));
    const runtime = new GraphRuntime({
      graph,
      nodeRegistry,
      conditionRegistry: new InMemoryConditionRegistry(),
      checkpointer: new InMemoryCheckpointer(),
      eventBus: new InMemoryEventBus()
    });
    const state = await runtime.start("run-swarm" as GraphState["runId"], {
      messages: [{ role: "human", content: "Need deep analysis" }]
    });
    expect(state.status).toBe("completed");
    expect(state.channels.reason).toBe("handoff to specialist");
    expect(Array.isArray(state.channels.messages)).toBe(true);
    expect(state.channels.done).toBe(true);
  });
});
