import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { createGraph, readInjected, rustEngineAvailable, type RunId } from "./index.js";

/**
 * Dynamic-message `send`: pre-queue inputs per node via `RunOptions.inbox`; each node
 * execution consumes the next (FIFO) via the `__injected` channel. Runs on both engines.
 */

const withEngine = (engine: "rust" | "ts") => {
  let saved: string | undefined;
  beforeEach(() => {
    saved = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = engine;
  });
  afterEach(() => {
    if (saved === undefined) {
      delete process.env.AILU_SDK_ENGINE;
    } else {
      process.env.AILU_SDK_ENGINE = saved;
    }
  });
};

/** A worker that records each injected input and loops until two are processed. */
const mapReduceGraph = () =>
  createGraph({ name: "send-fifo" })
    .channel("log", { type: "array", reducer: "append", default: [] as unknown[] })
    .channel("n", { type: "number", default: 0 })
    .node("worker", async (_input, state) => ({
      log: [readInjected(state)],
      n: ((state.channels.n as number) ?? 0) + 1
    }))
    .conditionalEdge("worker", "worker", "more", (state) => ((state.channels.n as number) ?? 0) < 2)
    .compile();

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu/graph-sdk — send / inbox (Rust engine)", () => {
  withEngine("rust");

  it("drains a pre-queued inbox FIFO across a cycle", async () => {
    const app = mapReduceGraph();
    expect(app.usesRustEngine).toBe(true);
    const result = await app.run(
      {},
      { runId: "run_send_rust" as RunId, inbox: { worker: ["first", "second"] } }
    );
    expect(result.status).toBe("completed");
    expect(result.channels.log).toEqual(["first", "second"]);
    // The injected value is never persisted as a channel.
    expect((result.channels as Record<string, unknown>).__injected).toBeUndefined();
  });
});

