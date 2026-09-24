import assert from "node:assert/strict";

// #region example
import { createGraph, finalAnswer, model, type AgentResult } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "review-files" })
  .channel("files", { type: "json", default: [] as string[] })
  // One sub-agent per item of `files`, run in parallel; results come back in input order.
  .mapAgents("review", {
    overChannel: "files",
    subAgent: { model: model.fast, prompt: { system: "Review this file. One sentence." } },
    joinAt: "reviews"
  })
  .compile();

const out = await app.run({ files: ["auth.ts", "billing.ts", "search.ts"] });
for (const review of out.channels.reviews as AgentResult[]) console.log(finalAnswer(review));
// #endregion example

assert.equal((out.channels.reviews as AgentResult[]).length, 3);
