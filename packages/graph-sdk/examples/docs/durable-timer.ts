import assert from "node:assert/strict";

// #region example
import { createGraph, readSuspendMeta, sleepUntil } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "reminder" })
  .channel("sent", { type: "boolean", default: false })
  // Suspend until a date. The engine records when to wake up; your scheduler resumes the run.
  .node("wait", async () => sleepUntil("2030-01-01T09:00:00Z"))
  .node("remind", async () => ({ sent: true }))
  .edge("wait", "remind")
  .compile();

const sleeping = await app.run();
console.log(readSuspendMeta(sleeping)); // { reason: "timer", wakeAt: "2030-01-01T09:00:00Z" }

// At wakeAt, your scheduler (cron, queue, ...) resumes the run.
const done = await app.resume(sleeping.runId);
console.log(done.channels.sent); // true
// #endregion example

assert.equal(readSuspendMeta(sleeping)?.wakeAt, "2030-01-01T09:00:00Z");
assert.equal(done.channels.sent, true);
