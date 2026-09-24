import { describe, expect, it } from "vitest";

import { createGraph, rustEngineAvailable, UnknownNodeError, type RunId } from "./index.js";

/**
 * `.fanOut()` runs a fixed set of branch nodes concurrently on the Rust engine and
 * joins their merged updates. Branches here are plain JS handlers (called back over the
 * napi seam) so the assertions are deterministic; the same wiring carries agentNode
 * branches for N parallel LLM calls.
 */

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu-ai/graph-sdk — fanOut (Rust engine)", () => {
  it("scatters to branches concurrently, merges in declared order, then joins", async () => {
    const app = createGraph({ name: "fanout-merge" })
      .channel("topic", { type: "string", default: "" })
      .channel("a", { type: "string", default: "" })
      .channel("b", { type: "string", default: "" })
      .channel("c", { type: "string", default: "" })
      .channel("summary", { type: "string", default: "" })
      .node("dispatch", async (_input, state) => ({ topic: String(state.channels.topic) }))
      .node("branchA", async (_input, state) => ({ a: `A:${state.channels.topic}` }))
      .node("branchB", async (_input, state) => ({ b: `B:${state.channels.topic}` }))
      .node("branchC", async (_input, state) => ({ c: `C:${state.channels.topic}` }))
      .node("collect", async (_input, state) => ({
        summary: `${state.channels.a}|${state.channels.b}|${state.channels.c}`
      }))
      .fanOut("dispatch", ["branchA", "branchB", "branchC"], "collect")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const result = await app.run({ topic: "x" }, { runId: "run_fanout" as RunId });

    expect(result.status).toBe("completed");
    // Every branch ran from the post-dispatch snapshot (topic = "x")...
    expect(result.channels.a).toBe("A:x");
    expect(result.channels.b).toBe("B:x");
    expect(result.channels.c).toBe("C:x");
    // ...and the join node observed all three merged updates.
    expect(result.channels.summary).toBe("A:x|B:x|C:x");
  });

  it("throws UnknownNodeError when the `from` node was never added", () => {
    expect(() =>
      createGraph({ name: "fanout-bad" })
        .channel("x", { type: "number", default: 0 })
        .node("only", async () => ({ x: 1 }))
        .fanOut("ghost", ["only"], "only")
    ).toThrow(UnknownNodeError);
  });
});
