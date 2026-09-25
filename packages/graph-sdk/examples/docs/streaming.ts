import assert from "node:assert/strict";

import { createGraph, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "explainer" })
  .channel("question", { type: "string", default: "" })
  .agentNode("answer", { model: model.anthropic("claude-sonnet-4-6"), prompt: { system: "Explain simply." } })
  .compile();

let tokens = "";
const nodes: string[] = [];

// #region messages
// Tokens as the model writes them.
for await (const event of app.stream({ question: "What is a checkpoint?" }, "messages")) {
  if (event.type === "message_delta") process.stdout.write(event.delta);
}
// #endregion messages

for await (const event of app.stream({ question: "What is a checkpoint?" }, "messages")) {
  if (event.type === "message_delta") tokens += event.delta;
}

// #region updates
// One event per finished node, with the channels it changed.
for await (const event of app.stream({ question: "What is a checkpoint?" }, "updates")) {
  if (event.type === "state_update") console.log(event.nodeId, Object.keys(event.delta));
}
// #endregion updates

for await (const event of app.stream({ question: "What is a checkpoint?" }, "updates")) {
  if (event.type === "state_update") nodes.push(String(event.nodeId));
}

assert.ok(tokens.length > 0);
assert.deepEqual(nodes, ["answer"]);
