import assert from "node:assert/strict";

// #region example
import { createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "research" })
  .channel("objective", { type: "string", default: "" })
  // A sub-agent in its own child graph: it sees only `objective`,
  // and only its report comes back (in `report`).
  .taskNode("dig", {
    subAgent: { model: model.balanced, prompt: { system: "Research the objective. Report in 3 bullets." } }
  })
  .compile();

const out = await app.run({ objective: "Compare durable execution engines" });
console.log(finalAnswer(out.channels.report));
// #endregion example

assert.equal(out.status, "completed");
