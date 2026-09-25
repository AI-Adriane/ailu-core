import assert from "node:assert/strict";

// #region example
import { createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "assistant" })
  .channel("question", { type: "string", default: "" })
  .agentNode("answer", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Answer in one sentence." }
  })
  .compile();

const out = await app.run({ question: "What is a checkpoint?" });
console.log(finalAnswer(out.channels.agentResult));
// #endregion example

assert.equal(out.status, "completed");
assert.ok(finalAnswer(out.channels.agentResult).length > 0);
