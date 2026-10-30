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
- From Python, `ailu.create_embeddings` and `ailu.create_vector_store` do the same through the
  engine, with the same file format ([Python](./python.md#embeddings-and-a-vector-store)).

### Custom embedding endpoints

Provider environment keys are used only with the selected provider's exact default base URL:
`https://api.mistral.ai/v1` or `https://api.openai.com/v1`. Any other string requires an explicit
endpoint key or injected transport. This includes a trailing slash, capitalization changes,
another port, or any other URL alias. URLs are not normalized into an implicit credential grant.
HTTP redirects are refused.

```ts
import { createEmbeddings } from "@ailu-ai/graph-sdk";

const embeddings = createEmbeddings({
  provider: "openai",
  baseUrl: "https://embeddings.example.com/v1",
  apiKey: process.env.EMBEDDINGS_ENDPOINT_KEY
});
```

Use a credential issued for that endpoint. Without it, client creation throws
`CustomEmbeddingsKeyRequiredError`; setting the provider's environment variable does not satisfy
this requirement. An explicit empty key still raises `MissingEmbeddingsKeyError`. A custom
`transport(body)` can implement its own authenticated or keyless client without reading provider
keys. The Rust API and Python SDK enforce the same destination rule in the shared engine.

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
