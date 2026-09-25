import assert from "node:assert/strict";

// #region example
import { createGraph, model, resumeCatalogGraph, runCatalogGraph, type GraphState } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "contract-review" })
  .channel("contract", { type: "string", default: "" })
  .agentNode("review", { model: model.balanced, prompt: { system: "List the risky clauses." } })
  .humanGate("legal-sign-off")
  .agentNode("summarize", {
    model: model.fast,
    prompt: { system: "Summarize the review for the customer." },
    outputChannel: "summary"
  })
  .edge("review", "legal-sign-off")
  .edge("legal-sign-off", "summarize")
  .compile();

// Process A: run until the gate, then save the state (a database row, a file, a queue message).
const paused = await runCatalogGraph(app.definition, { initialData: { contract: "..." } });
console.log(paused.status); // "suspended"
const saved = JSON.stringify(paused.state);

// Process B, hours later, maybe on another machine: load the state and resume.
const done = await resumeCatalogGraph(app.definition, JSON.parse(saved) as GraphState);
console.log(done.status); // "completed"
// #endregion example

assert.equal(paused.status, "suspended");
assert.equal(done.status, "completed");
