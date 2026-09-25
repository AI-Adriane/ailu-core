import assert from "node:assert/strict";

// #region example
import { createGraph, model } from "@ailu-ai/graph-sdk";

const ticketSchema = {
  type: "object",
  properties: {
    category: { type: "string", enum: ["billing", "bug", "other"] },
    urgent: { type: "boolean" }
  },
  required: ["category", "urgent"],
  additionalProperties: false
};

const app = createGraph({ name: "ticket-triage" })
  .channel("ticket", { type: "string", default: "" })
  .agentNode("classify", {
    model: model.openai("gpt-4o"),
    prompt: { system: "Classify the support ticket." },
    // The agent must answer with JSON that matches the schema.
    middleware: [{ kind: "structuredOutput", params: { schema: ticketSchema, mode: "lenient" } }]
  })
  .compile();

const out = await app.run({ ticket: "I was charged twice this month!" });
const ticket = out.channels.agentResult.structuredOutput as { category: string; urgent: boolean } | undefined;
console.log(ticket); // { category: "billing", urgent: true }
// #endregion example

assert.equal(out.status, "completed");
