import assert from "node:assert/strict";

// #region example
import { createGraph, model, runCatalogGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "long-job" })
  .node("step1", async () => ({})) // a plain step: the catalog runner runs your binding for it
  .agentNode("step2", { model: model.fast, prompt: { system: "Do the job." }, outputChannel: "work" })
  .edge("step1", "step2")
  .compile();

// Abort the controller to stop the run. The engine finishes the node in flight,
// saves its checkpoint, and returns with status "cancelled".
const controller = new AbortController();
const outcome = await runCatalogGraph(app.definition, {
  signal: controller.signal,
  nodes: [
    {
      id: "step1",
      execute: () => {
        controller.abort(); // e.g. the user clicks Stop while step1 runs
        return {};
      }
    }
  ]
});
console.log(outcome.status); // "cancelled": step1 finished, step2 never started
// #endregion example

assert.equal(outcome.status, "cancelled");
// step2 never ran, and a resume would start there.
assert.equal(outcome.state.channels.work, null);
assert.equal(outcome.state.currentNodeId, "step2");
