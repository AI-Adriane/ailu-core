// Quickstart: an agent drafts a reply, a human approves it, the run finishes.
// Run it: npx tsx quickstart.ts   (set ANTHROPIC_API_KEY, or AILU_LLM_MOCK=1 to run offline)
import assert from "node:assert/strict";

// #region example
import { createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "refund-desk" })
  .channel("request", { type: "string", default: "" })
  .channel("reply", { type: "string", default: "" })
  .agentNode("draft", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Draft a one-sentence reply to the customer's refund request." }
  })
  .humanGate("review") // the run stops here until a human approves
  .node("send", async (_input, state) => ({ reply: finalAnswer(state.channels.agentResult) }))
  .edge("draft", "review")
  .edge("review", "send")
  .compile();

// 1. Run: the agent drafts, then the run suspends at the gate.
const paused = await app.run({ request: "Please refund order #1024." });
console.log(paused.status); // "suspended"
console.log(finalAnswer(paused.channels.agentResult)); // the draft a human reviews

// 2. A human approved: resume from the checkpoint.
const done = await app.resume(paused.runId);
console.log(done.status); // "completed"
console.log(done.channels.reply);
// #endregion example

assert.equal(paused.status, "suspended");
assert.equal(done.status, "completed");
assert.ok(done.channels.reply.length > 0);
