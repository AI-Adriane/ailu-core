---
title: "RAG and retrieval"
description: "Retrieve relevant documents and let an agent answer from them, with keyword search or embeddings."
---

# RAG and retrieval

Retrieval-augmented generation (RAG) has two steps: find the documents that match the question,
then let an agent answer from them. In Ailu each step is a node. The retriever writes its matches
to a channel; the agent reads that channel with the rest of the state.

## Start with keyword search

BM25 ranks documents by the words they share with the question. It needs no API key and no
vector store, and it is hard to beat on short, precise corpora (FAQs, policies, product docs).

```ts file=packages/graph-sdk/examples/docs/rag-keyword.ts region=example
```

Each match is `{ id, content, score }`.

## Search by meaning with embeddings

When questions use different words than your documents, embed both and search by similarity.
`semanticRetriever` embeds the documents once, stores the vectors, and returns the closest
matches.

```ts file=packages/graph-sdk/examples/docs/rag-semantic.ts region=example
```

- `createEmbeddings({ provider: "openai" | "mistral" })` reads `OPENAI_API_KEY` or
  `MISTRAL_API_KEY`, and throws when the key is missing.
- `createVectorStore({ persistPath })` keeps the vectors in a JSON file. Omit `persistPath` for
  an in-memory store. For large corpora, implement the `VectorStore` interface over your
  database and pass it as `store`.

## Improve the ranking

| Component | What it does |
| --- | --- |
| `components.reranker` | Re-scores the matches against the question with a cross-encoder at `AILU_RERANK_ENDPOINT`. Without it, keeps the order. |
| `components.mergeRanker` | Fuses the results of several retrievers (keyword + semantic) into one ranking, with Reciprocal Rank Fusion. |
| `components.documentSplitter` | Splits a long text into chunks, by characters or sentences, before you index it. |
| `components.answerBuilder` | Assembles the final answer text, with numbered citations if you want them. |

All components are listed with their parameters in [Components](../reference/components.md).

## Next

- A complete question-answering app: [Document Q&A](../examples/document-qa.md).
