import assert from "node:assert/strict";

// #region example
import { createGraph, readSignal, waitForSignal } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "order" })
  .channel("paid", { type: "boolean", default: false })
  // The run suspends here until your system delivers the "payment" signal.
  .node("await-payment", async () => waitForSignal("payment"))
  .node("ship", async (_input, state) => ({ paid: readSignal(state, "payment") !== undefined }))
  .edge("await-payment", "ship")
  .compile();

const waiting = await app.run();
console.log(waiting.status); // "suspended"

// Later, a webhook arrives: deliver the signal and the run continues.
const done = await app.signal(waiting.runId, "payment", { amount: 42 });
console.log(done.status, done.channels.paid); // "completed" true
// #endregion example

assert.equal(waiting.status, "suspended");
assert.equal(done.status, "completed");
assert.equal(done.channels.paid, true);
