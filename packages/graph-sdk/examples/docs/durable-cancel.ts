import assert from "node:assert/strict";

// #region example
import { createGraph, model, runCatalogGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "long-job" })
  .agentNode("step1", { model: model.fast, prompt: { system: "Plan the job." }, outputChannel: "plan" })
  .agentNode("step2", { model: model.fast, prompt: { system: "Do the job." }, outputChannel: "work" })
  .edge("step1", "step2")
  .compile();

// Abort the controller to stop the run. The engine finishes the node in flight,
// saves its checkpoint, and returns with status "cancelled".
const controller = new AbortController();
const outcome = await runCatalogGraph(app.definition, {
  signal: controller.signal,
  onEvent: (event) => {
    if (event.type === "node_completed") controller.abort(); // e.g. the user clicked Stop
  }
});
console.log(outcome.status); // "cancelled"
// #endregion example

assert.equal(outcome.status, "cancelled");
