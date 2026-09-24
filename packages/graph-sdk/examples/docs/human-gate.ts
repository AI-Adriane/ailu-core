import assert from "node:assert/strict";

// #region example
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .channel("draft", { type: "string", default: "" })
  .channel("published", { type: "boolean", default: false })
  .node("write", async () => ({ draft: "Release notes for 2.0" }))
  .humanGate("review")
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const paused = await app.run();
console.log(paused.status); // "suspended"
console.log(paused.currentNodeId); // "review"

// What is the run waiting for? Useful in a UI, a log, or for an AI agent.
console.log(app.explain(paused.runId).summary);

// After a human approves in your app, resume on the same CompiledGraph.
const done = await app.resume(paused.runId);
console.log(done.status, done.channels.published); // "completed" true
// #endregion example

assert.equal(paused.status, "suspended");
assert.equal(String(paused.currentNodeId), "review");
assert.equal(done.channels.published, true);
