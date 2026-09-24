import assert from "node:assert/strict";

// #region example
import { components, createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

const docs = [
  { id: "refunds", content: "Refunds are issued within 5 business days of approval." },
  { id: "shipping", content: "Orders ship within 24 hours on weekdays." },
  { id: "returns", content: "Items can be returned within 30 days." }
];

const app = createGraph({ name: "support-faq" })
  .channel("question", { type: "string", default: "" })
  .channel("context", { type: "json", default: [] as Array<{ id: string; content: string; score: number }> })
  // BM25 keyword search: no API key, no vector store. Top matches land in `context`.
  .component("retrieve", components.bm25Retriever({ query: "question", into: "context", k: 2, docs }))
  // The agent sees the question and the retrieved context.
  .agentNode("answer", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Answer from the context only. Say 'I don't know' otherwise." }
  })
  .edge("retrieve", "answer")
  .compile();

const out = await app.run({ question: "How long do refunds take?" });
console.log(out.channels.context); // [{ id: "refunds", content: "...", score: ... }, ...]
console.log(finalAnswer(out.channels.agentResult));
// #endregion example

assert.equal(out.channels.context[0]?.id, "refunds");
