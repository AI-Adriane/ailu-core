import { describe, expect, it } from "vitest";

import { createGraph, DuplicateNodeError, GraphCompileError, MissingHandlerError } from "./index.js";
import type { GraphState, RunEvent } from "./index.js";

describe("@ailu/graph-sdk", () => {
  it("compiles and runs a single-node graph", async () => {
    const app = createGraph({ name: "greeter" })
      .node("hello", async (_input, state) => ({
        greeting: `Hello, ${String((state.channels as Record<string, unknown>).name)}!`
      }))
      .compile();

    const result = await app.run({ name: "Ada" });

    expect(result.status).toBe("completed");
    expect((result.channels as Record<string, unknown>).greeting).toBe("Hello, Ada!");
  });

  it("infers channel value types through to the handler and the run result (no casts)", async () => {
    const app = createGraph({ name: "typed" })
      .channel("count", { type: "number", default: 0 })
      .channel("label", { type: "string", default: "" })
      .node("bump", async (_input, state) => {
        // state.channels.count is statically `number` here — no cast needed.
        const next = state.channels.count + 1;
        return { count: next, label: `n=${next}` };
      })
      .compile();

    const result = await app.run({ count: 10 });
    // result.channels.count is statically `number`; result.channels.label is `string`.
    const count: number = result.channels.count;
    const label: string = result.channels.label;

    expect(count).toBe(11);
    expect(label).toBe("n=11");
  });

  it("defaults the entry node to the first node added", async () => {
    const app = createGraph({ name: "two-step" })
      .channel("count", { type: "number", default: 0 })
      .node("first", async (_input, state) => ({
        count: ((state.channels as Record<string, number>).count ?? 0) + 1
      }))
      .node("second", async (_input, state) => ({
        count: ((state.channels as Record<string, number>).count ?? 0) + 10
      }))
      .edge("first", "second")
      .compile();

    const result = await app.run();

    expect(app.definition.entryNodeId).toBe("first");
    expect((result.channels as Record<string, number>).count).toBe(11);
  });

  it("routes through a named conditional edge (never an eval'd string)", async () => {
    const reached: string[] = [];
    const app = createGraph({ name: "router" })
      .channel("score", { type: "number", default: 0 })
      .node("start", async () => ({ score: 5 }))
      .node("high", async () => {
        reached.push("high");
        return {};
      })
      .node("low", async () => {
        reached.push("low");
        return {};
      })
      .conditionalEdge("start", "high", "isHigh", (state) => Number((state.channels as Record<string, number>).score) >= 3)
      .conditionalEdge("start", "low", "isLow", (state) => Number((state.channels as Record<string, number>).score) < 3)
      .compile();

    await app.run();

    expect(reached).toEqual(["high"]);
  });

  it("routes to a declared error edge once retries are exhausted, instead of failing the run (ADR 0076)", async () => {
    const reached: string[] = [];
    const app = createGraph({ name: "error-branch" })
      .node("flaky", {
        type: "action",
        handler: async () => {
          throw new Error("always fails");
        },
        retryPolicy: { maxAttempts: 2, backoffMs: 0 }
      })
      .node("handler", async () => {
        reached.push("handler");
        return {};
      })
      .errorEdge("flaky", "handler")
      .compile();

    const result = await app.run();

    expect(result.status).toBe("completed");
    expect(reached).toEqual(["handler"]);
  });

  it("suspends at a human gate and resumes from the checkpoint", async () => {
    const app = createGraph({ name: "approval-flow" })
      .channel("approved", { type: "boolean", default: false })
      .node("draft", async () => ({ approved: false }))
      .humanGate("review")
      .node("publish", async () => ({ approved: true }))
      .edge("draft", "review")
      .edge("review", "publish")
      .compile();

    const suspended = await app.run({}, { runId: "run_fixed_1" as never });
    expect(suspended.status).toBe("suspended");

    const resumed = await app.resume(suspended.runId);
    expect(resumed.status).toBe("completed");
    expect((resumed.channels as Record<string, boolean>).approved).toBe(true);
  });

  it("emits run lifecycle events to subscribers", async () => {
    const events: RunEvent[] = [];
    const app = createGraph({ name: "observed" })
      .node("only", async () => ({}))
      .compile();

    app.onEvent((event) => events.push(event));
    await app.run();

    expect(events.some((event) => event.type === "node_started")).toBe(true);
    expect(events.some((event) => event.type === "node_completed")).toBe(true);
  });

  it("safeCompile reports validation failures instead of throwing", () => {
    const result = createGraph({ name: "broken" })
      .node("a", async () => ({}))
      .edge("a", "ghost")
      .safeCompile();

    // 'ghost' is referenced but never added — caught by the engine validator.
    expect(result.success).toBe(false);
    if (!result.success) {
      expect(result.error).toBeInstanceOf(GraphCompileError);
      expect(result.error.errors.some((e) => e.code === "INVALID_EDGE_REFERENCE")).toBe(true);
    }
  });

  it("compile throws GraphCompileError when the entry node is missing", () => {
    const builder = createGraph({ name: "no-entry" }).entry("nope");
    expect(() => builder.compile()).toThrow(GraphCompileError);
  });

  it("rejects duplicate node ids", () => {
    const builder = createGraph({ name: "dupes" }).node("a", async () => ({}));
    expect(() => builder.node("a", async () => ({}))).toThrow(DuplicateNodeError);
  });

  it("rejects action nodes without a handler", () => {
    const builder = createGraph({ name: "handlerless" });
    expect(() => builder.node("a", { type: "action" })).toThrow(MissingHandlerError);
  });

  it("streams state values as the graph executes", async () => {
    const app = createGraph({ name: "streamer" })
      .channel("n", { type: "number", default: 0 })
      .node("a", async () => ({ n: 1 }))
      .node("b", async (_input, state) => ({ n: Number((state.channels as Record<string, number>).n) + 1 }))
      .edge("a", "b")
      .compile();

    const states: GraphState[] = [];
    for await (const event of app.stream({}, "values")) {
      if (event.type === "state_value") {
        states.push(event.state);
      }
    }

    expect(states.at(-1) !== undefined).toBe(true);
    expect((states.at(-1)?.channels as Record<string, number>).n).toBe(2);
  });
});
