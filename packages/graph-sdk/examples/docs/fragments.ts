// Short fragments shown inline in the documentation, one region each. They run like the other
// snippets, so every line of code in the docs is typechecked and executed.
import assert from "node:assert/strict";

import {
  AiluSdkError,
  components,
  createGraph,
  model,
  runCatalogGraph,
  rustEngineAvailable,
  serveInspector
} from "@ailu-ai/graph-sdk";

// #region engine-check
console.log(rustEngineAvailable()); // true
// #endregion engine-check

const builder = createGraph({ name: "fragments" }).channel("question", { type: "string", default: "" });

// #region visible-channels
builder.agentNode("answer", {
  model: model.anthropic("claude-sonnet-4-6"),
  prompt: { system: "Answer the question." },
  visibleChannels: ["question", "context"]
});
// #endregion visible-channels

const app = builder.compile();
const paused = await app.run({ question: "hi" });

// #region skills
await runCatalogGraph(app.definition, { skills: [/* SkillRecord, ... */] });
// #endregion skills

// #region component
createGraph({ name: "greet" })
  .channel("name", { type: "string", default: "" })
  .component("prompt", components.promptBuilder({ template: "Hello {{name}}!", into: "prompt" }));
// #endregion component

// #region custom-endpoint
model.openaiCompatible({
  baseURL: "https://gateway.example.com/v1",
  model: "llama-3.1-70b",
  apiKeyEnv: "GATEWAY_API_KEY" // sent as a Bearer token; omit for a keyless server
});
// #endregion custom-endpoint

// #region errors
try {
  createGraph({ name: "broken" }).edge("a", "b").compile();
} catch (error) {
  if (error instanceof AiluSdkError) console.error(error.format());
}
// #endregion errors

// #region explain
const explanation = app.explain(paused.runId);
console.log(explanation.summary);
// #endregion explain

export const inspect = async () => {
  // #region inspector
  const inspector = await serveInspector(app, { question: "What is a checkpoint?" });
  console.log(inspector.url);
  // #endregion inspector
};

assert.equal(rustEngineAvailable(), true);
assert.equal(typeof explanation.summary, "string");
