import assert from "node:assert/strict";

// #region example
import { createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "pros-and-cons" })
  .channel("decision", { type: "string", default: "" })
  .channel("summary", { type: "string", default: "" })
  .node("start", async () => ({}))
  // Two agents run in parallel on the same state. Each writes its own channel.
  .agentNode("pros", {
    model: model.fast,
    prompt: { system: "List the strongest argument FOR the decision." },
    outputChannel: "pros"
  })
  .agentNode("cons", {
    model: model.fast,
    prompt: { system: "List the strongest argument AGAINST the decision." },
    outputChannel: "cons"
  })
  .node("merge", async (_input, state) => ({
    summary: `For: ${finalAnswer(state.channels.pros)} / Against: ${finalAnswer(state.channels.cons)}`
  }))
  // start fans out to both agents; the run continues at merge once both are done.
  .fanOut("start", ["pros", "cons"], "merge")
  .compile();

const out = await app.run({ decision: "Move the team to a four-day week" });
console.log(out.channels.summary);
// #endregion example

assert.equal(out.status, "completed");
assert.ok(out.channels.summary.startsWith("For: "));
