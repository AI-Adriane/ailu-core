import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import type { RunExplanation } from "@ailu-ai/graph-sdk";
import {
  MAX_EXPLANATION_BYTES,
  MAX_RETAINED_BYTES,
  MAX_SUSPENDED_RUNS,
  SUSPENDED_RUN_TTL_MS,
  SuspendedRunSnapshots
} from "./suspended-runs.mjs";

afterEach(() => vi.useRealTimers());

const explanation = (runId: string): RunExplanation => ({
  runId,
  status: "suspended",
  currentNode: "review",
  summary: "Awaiting human approval",
  channels: ["draft"],
  suspended: { reason: "human_gate", node: "review", nextAction: "Use a trusted host" }
});

test("snapshots are detached on insertion and retrieval", () => {
  const cache = new SuspendedRunSnapshots();
  const original = explanation("run");
  assert.equal(cache.set("run", original), true);
  original.channels.push("later mutation");
  const first = cache.get("run")!;
  assert.deepEqual(first, explanation("run"));
  first.channels.push("mutated read");
  assert.deepEqual(cache.get("run"), explanation("run"));
  assert.equal(cache.get("missing"), undefined);
});

test("repeated suspensions stay within the count limit without reads refreshing FIFO", () => {
  const cache = new SuspendedRunSnapshots();
  for (let i = 0; i < MAX_SUSPENDED_RUNS * 3; i++) {
    cache.get("0");
    assert.equal(cache.set(String(i), explanation(String(i))), true);
  }
  const retained = Array.from({ length: MAX_SUSPENDED_RUNS * 3 }, (_, i) => cache.get(String(i)));
  assert.equal(retained.filter(Boolean).length, MAX_SUSPENDED_RUNS);
  assert.equal(retained[0], undefined);
  assert.ok(retained.at(-1));
});

test("UTF-8 bytes limit individual snapshots and the entire cache", () => {
  const cache = new SuspendedRunSnapshots();
  const large = { ...explanation("large"), channels: ["é".repeat(20_000)] };
  const bytes = Buffer.byteLength(JSON.stringify(large));
  assert.ok(bytes < MAX_EXPLANATION_BYTES);
  for (let i = 0; i < MAX_SUSPENDED_RUNS; i++) assert.equal(cache.set(String(i), large), true);
  const retained = Array.from({ length: MAX_SUSPENDED_RUNS }, (_, i) => cache.get(String(i)));
  assert.equal(retained.filter(Boolean).length, Math.floor(MAX_RETAINED_BYTES / bytes));

  // Fewer than 64K characters can still exceed 64KiB in UTF-8.
  const oversized = { ...large, channels: ["é".repeat(MAX_EXPLANATION_BYTES / 2)] };
  assert.equal(cache.set("oversized", oversized), false);
  assert.equal(cache.get("oversized"), undefined);
  assert.deepEqual(cache.get(String(MAX_SUSPENDED_RUNS - 1)), large);

  const exact = { ...explanation("exact"), channels: [""] };
  exact.channels[0] = "x".repeat(MAX_EXPLANATION_BYTES - Buffer.byteLength(JSON.stringify(exact)));
  assert.equal(cache.set("exact", exact), true);
  exact.channels[0] += "x";
  assert.equal(cache.set("exact", exact), false);
  assert.equal(cache.get("exact"), undefined);
});

test("fixed TTL releases idle snapshots and reads cannot extend expiry", () => {
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
  const cache = new SuspendedRunSnapshots();
  cache.set("first", explanation("first"));
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS - 1);
  assert.ok(cache.get("first"));
  cache.set("second", explanation("second"));
  vi.advanceTimersByTime(1);
  assert.equal(cache.get("first"), undefined);
  assert.ok(cache.get("second"));
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS);
  assert.equal(cache.get("second"), undefined);
  // Inspect the private storage to prove timer cleanup occurs even with no reads.
  cache.set("idle", explanation("idle"));
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS);
  assert.equal((cache as unknown as { entries: Map<string, unknown> }).entries.size, 0);
  assert.equal((cache as unknown as { retainedBytes: number }).retainedBytes, 0);
});

test("lookup checks expiry even before timer callbacks are dispatched", () => {
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
  const cache = new SuspendedRunSnapshots();
  cache.set("run", explanation("run"));
  vi.setSystemTime(Date.now() + SUSPENDED_RUN_TTL_MS);
  assert.equal(cache.get("run"), undefined);
});

test("replacing an entry clears its old expiry timer and byte accounting", () => {
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
  const cache = new SuspendedRunSnapshots();
  cache.set("run", explanation("run"));
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS / 2);
  const replacement = { ...explanation("run"), channels: ["replacement"] };
  cache.set("run", replacement);
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS / 2);
  assert.deepEqual(cache.get("run"), replacement);
  vi.advanceTimersByTime(SUSPENDED_RUN_TTL_MS / 2);
  assert.equal(cache.get("run"), undefined);
  assert.equal((cache as unknown as { retainedBytes: number }).retainedBytes, 0);
});
