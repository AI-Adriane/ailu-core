import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  createGraph,
  readSignal,
  readSuspendMeta,
  rustEngineAvailable,
  sleepUntil,
  waitForSignal,
  type RunId
} from "./index.js";

/**
 * Durable timers + external signals (ADR 0009) run on the **Rust engine** (the
 * production runtime). These exercise the end-to-end suspend → resume / signal cycle
 * through the napi bridge; the TS fallback does not model them (guarded below).
 */

const withRustEngine = () => {
  let saved: string | undefined;
  beforeEach(() => {
    saved = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = "rust";
  });
  afterEach(() => {
    if (saved === undefined) {
      delete process.env.AILU_SDK_ENGINE;
    } else {
      process.env.AILU_SDK_ENGINE = saved;
    }
  });
};

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu-ai/graph-sdk — durable timers + signals (Rust engine)", () => {
  withRustEngine();

  it("a durable timer suspends, exposes wakeAt, then advances on resume", async () => {
    const app = createGraph({ name: "timer" })
      .channel("step", { type: "string", default: "" })
      .channel("done", { type: "boolean", default: false })
      .node("wait", async () => sleepUntil("2026-01-01T00:00:00Z", { step: "sleeping" }))
      .node("after", async () => ({ done: true }))
      .edge("wait", "after")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const suspended = await app.run({}, { runId: "run_timer" as RunId });
    expect(suspended.status).toBe("suspended");
    // The node's update applied before the run parked.
    expect(suspended.channels.step).toBe("sleeping");
    const meta = readSuspendMeta(suspended);
    expect(meta?.reason).toBe("timer");
    expect(meta?.wakeAt).toBe("2026-01-01T00:00:00Z");

    // The scheduler resumes at wakeAt → advance past the timer node (one-shot).
    const resumed = await app.resume(suspended.runId);
    expect(resumed.status).toBe("completed");
    expect(resumed.channels.done).toBe(true);
  });

  it("an external signal suspends, then resumes with the delivered payload", async () => {
    const app = createGraph({ name: "signal" })
      .channel("received", { type: "string", default: "" })
      .node("wait", async () => waitForSignal("approval"))
      .node("after", async (_input, state) => ({
        received: String(readSignal(state, "approval") ?? "")
      }))
      .edge("wait", "after")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const suspended = await app.run({}, { runId: "run_signal" as RunId });
    expect(suspended.status).toBe("suspended");
    expect(readSuspendMeta(suspended)?.reason).toBe("signal");
    expect(readSuspendMeta(suspended)?.awaitingSignal).toBe("approval");

    const resumed = await app.signal(suspended.runId, "approval", "yes");
    expect(resumed.status).toBe("completed");
    expect(resumed.channels.received).toBe("yes");
  });

  it("a signal-or-timeout wakes on the timer when no signal arrives", async () => {
    const app = createGraph({ name: "sot" })
      .channel("viaSignal", { type: "boolean", default: false })
      .node("wait", async () => waitForSignal("approval", { wakeAt: "2026-01-01T00:00:00Z" }))
      .node("after", async (_input, state) => ({
        viaSignal: readSignal(state, "approval") !== undefined
      }))
      .edge("wait", "after")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const suspended = await app.run({}, { runId: "run_sot" as RunId });
    expect(suspended.status).toBe("suspended");
    const meta = readSuspendMeta(suspended);
    expect(meta?.reason).toBe("signal");
    expect(meta?.wakeAt).toBe("2026-01-01T00:00:00Z");

    // Timeout path: a plain resume (no signal) still advances; downstream sees no signal.
    const resumed = await app.resume(suspended.runId);
    expect(resumed.status).toBe("completed");
    expect(resumed.channels.viaSignal).toBe(false);
  });
});
