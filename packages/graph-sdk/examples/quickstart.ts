/**
 * Ailu quickstart — a resumable graph with a human-approval gate, in ~20 lines.
 *
 * The exact behaviour shown here is exercised by `src/index.test.ts`, so this
 * file stays honest: if the SDK changes, the test breaks.
 */
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .channel("draft", { type: "string", default: "" })
  .channel("approved", { type: "boolean", default: false })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review") // suspends the run cleanly; resume it after approval
  .node("publish", async () => ({ approved: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

// 1) Start the run — it suspends at the human gate.
const suspended = await app.run();
console.log(suspended.status); // "suspended"

// 2) A human approves out-of-band, then you resume from the latest checkpoint.
const done = await app.resume(suspended.runId);
console.log(done.status); // "completed"
// `approved` is statically typed `boolean` here — channels flow through to the result.
console.log(done.channels.approved); // true
