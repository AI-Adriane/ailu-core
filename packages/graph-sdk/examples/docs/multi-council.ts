import assert from "node:assert/strict";

// #region example
import { council, finalAnswer, model, runCatalogGraph, type AgentResult } from "@ailu-ai/graph-sdk";

const seat = (system: string) => ({ model: model.balanced, prompt: { system } });

const definition = council({
  // Each member answers on its own...
  members: [seat("Answer as a lawyer."), seat("Answer as an engineer."), seat("Answer as a CFO.")],
  // ...reviewers rank the answers without knowing who wrote them (labels A, B, C)...
  reviewers: [seat("Rank the answers A, B, C from best to worst.")],
  // ...and a chair writes the final answer from the ranking.
  chair: seat("Write the final answer from the best-ranked answers."),
  humanGate: false // true: stop for a human before the chair decides
});

const outcome = await runCatalogGraph(definition, {
  initialData: { query: "Should we open-source our SDK?" }
});
console.log(finalAnswer(outcome.state.channels.answer as AgentResult));
// #endregion example

assert.equal(outcome.status, "completed");
