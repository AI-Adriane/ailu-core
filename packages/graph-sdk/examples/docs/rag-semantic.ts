import assert from "node:assert/strict";

// #region example
import { createEmbeddings, createGraph, createVectorStore, model, semanticRetriever } from "@ailu-ai/graph-sdk";

export const buildHelpdesk = () =>
  createGraph({ name: "helpdesk" })
    .channel("question", { type: "string", default: "" })
    .node(
      "retrieve",
      semanticRetriever({
        queryFrom: "question",
        into: "context",
        k: 4,
        docs: [
          { id: "refunds", content: "Refunds are issued within 5 business days of approval." },
          { id: "shipping", content: "Orders ship within 24 hours on weekdays." }
        ],
        // Embeddings need OPENAI_API_KEY (or provider: "mistral" with MISTRAL_API_KEY).
        embeddings: createEmbeddings({ provider: "openai" }),
        // Vectors are kept in a JSON file, so documents are embedded once.
        store: createVectorStore({ persistPath: "./vectors.json" })
      })
    )
    .agentNode("answer", {
      model: model.openai("gpt-4o"),
      prompt: { system: "Answer from the context only." }
    })
    .edge("retrieve", "answer")
    .compile();
// #endregion example

// Needs a real embeddings key, so the test only checks that the code is well formed.
assert.equal(typeof buildHelpdesk, "function");
