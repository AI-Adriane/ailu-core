---
sidebar_position: 1
title: Retrievers overview
description: The retriever and reranker graph components — lexical (BM25, keyword), mock-embedding and real-embedding semantic retrieval, deterministic reranking, and RRF fusion — added with .component(...).
---

# Retrievers overview

Retrieval in Adriane is **graph components**, not a separate runtime. A retriever node reads a query
from a channel, scores a corpus, and writes a top-`k` array of `{ id, content, score }` to another
channel. A reranker node reorders such an array. You wire them like any other node with
`.component(id, descriptor)` on a `createGraph(...)` builder, so retrieval composes with routing,
human gates, and checkpointing for free.

Every component below is **pure and deterministic** — same input channels, same output, every time.
None of them call a provider: embedding is a separate, upstream concern (see
[Embeddings](#embeddings)). That is what keeps retrieval reproducible and replayable across runs.

:::note Authoritative implementation is the Rust crate
These components live in `crates/components` and are reached through `@adriane-ai/napi`, consumed via
`@adriane-ai/graph-sdk`. The TypeScript `@adriane-ai/rag-pipeline` package (`Retriever`,
`LLMReranker`) is a deprecated fallback — see ADR 0003. New code builds retrieval through
`@adriane-ai/graph-sdk`.
:::

## Usage

Add a retriever and a reranker as nodes, seed the query and corpus on channels, and run:

```ts
import { createGraph, components } from "@adriane-ai/graph-sdk";

const app = createGraph({ name: "retrieve-and-rank" })
  .channel("query", { type: "string", default: "" })
  .channel("hits", { type: "json", default: [] as unknown[] })
  .channel("ranked", { type: "json", default: [] as unknown[] })
  .component(
    "retrieve",
    components.bm25Retriever({
      query: "query",
      into: "hits",
      k: 5,
      docs: [
        { id: "checkpointing", content: "Adriane checkpoints after every node completion." },
        { id: "gates", content: "A human gate suspends the run until someone resumes it." }
      ]
    })
  )
  .component("rerank", components.reranker({ from: "hits", into: "ranked", query: "query" }))
  .compile();

const result = await app.run({ query: "how does resumability work" });
// result.channels.ranked → [{ id, content, score }, …] highest first
```

`docs` is the inline corpus for the lexical and mock retrievers. The query is read from the named
channel at run time; if that channel is empty, the literal value of `query` is used as the query
text instead.

## Retrievers

All retrievers emit a top-`k` array of `{ id, content, score }`, sorted descending, with stable
input order breaking ties.

| Kind | Scoring | Corpus source | Factory helper |
| --- | --- | --- | --- |
| `bm25Retriever` | Lexical BM25 (IDF · saturated TF · length norm) | inline `docs` | `components.bm25Retriever` |
| `keywordRetriever` | Fraction of distinct query terms present, in `[0,1]` | inline `docs` | `components.keywordRetriever` |
| `retriever` | Cosine over a **deterministic mock embedder** | inline `docs` | `components.retriever` |
| `semanticRetriever` | Cosine over **real, pre-computed embeddings** | channels | Planned (see below) |

### `bm25Retriever`

Lexical BM25 ranking. Tokenizes on non-alphanumeric boundaries; `k1` is term-frequency saturation,
`b` is length-normalization.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (literal fallback when empty). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to rank. |
| `k` | `number` | `4` | Number of results to keep. |
| `k1` | `number` | `1.2` | BM25 term-frequency saturation. |
| `b` | `number` | `0.75` | BM25 length-normalization. |

### `keywordRetriever`

Keyword-overlap ranking: score is `|matched query terms| / |query terms|`, a simple explainable
alternative to BM25. Docs with no overlap score `0`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (literal fallback). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to rank. |
| `k` | `number` | `4` | Number of results to keep. |

### `retriever`

Scores docs against the query with a **deterministic mock embedder** (a 4-bucket character-count
vector) + cosine similarity. The mock embedder makes retrieval run keyless and reproducible in
dev/test; it is not semantically meaningful. For genuine semantics use `semanticRetriever` (Planned)
once embeddings exist on the channel.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (literal fallback). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to score. |
| `k` | `number` | `4` | Number of results to keep. |

### `semanticRetriever` — Planned factory helper

The **component kind exists** in the crate and the [component catalog](/docs/reference/component-catalog):
it ranks PRE-EMBEDDED chunks by cosine similarity to a PRE-EMBEDDED query, both supplied on channels,
keeping the top-`k`. Unlike `retriever`, it consumes real embeddings produced upstream by the gateway
(e.g. Mistral) — the host seeds `chunksFrom` with a namespace's persisted KB and `queryEmbeddingFrom`
with the embedded query. The component owns only the cosine ranking.

What is **not yet shipped** is a typed `components.semanticRetriever(...)` helper in
`@adriane-ai/graph-sdk` (the factory currently exposes `retriever`, `bm25Retriever`,
`keywordRetriever`, `reranker`, `mergeRanker`). Until that lands, reach the `semanticRetriever` kind
through graph YAML / the crate registry.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `queryEmbeddingFrom` | `string` | — | Channel holding the query's embedding (`number[]`). |
| `chunksFrom` | `string` | — | Channel holding the corpus: `{ id, content, embedding }[]`. |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `k` | `number` | `4` | Number of results to keep. |

## Rerankers

### `reranker`

Reorder a retrieval-result array. Without a cross-encoder (below) it sorts by each item's existing
`score`, so the upstream (BM25 / fusion) ranking is kept — a `query` channel is accepted but only
used by the cross-encoder. The score is written back onto each item with a provenance step. Stable
sort keeps input order on ties; items missing a `score` are tolerated as score `0`.

**Cross-encoder reranking (since 1.16.0).** Set `ADRIANE_RERANK_ENDPOINT` to a self-hostable,
EU-sovereign rerank service (a HuggingFace [TEI](https://github.com/huggingface/text-embeddings-inference)
endpoint serving `BAAI/bge-reranker-v2-m3`) and the `reranker` node re-scores its candidates through
that real cross-encoder — the single biggest retrieval-precision lever. Add
`ADRIANE_RERANK_API_KEY` for an authenticated endpoint (sent as `Authorization: Bearer`; omit for an
unauthenticated self-hosted TEI). With no endpoint the node is an **identity passthrough** that
preserves the upstream ranking (e.g. `mergeRanker`'s RRF order) — never a mock rescoring — and a
rerank error is fail-open (the upstream order is kept). The rerank call routes through the
`llm-gateway` seam (the only crate allowed to reach an external service); the engine stays lean.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the array to reorder. |
| `into` | `string` | — | Channel receiving the reordered array. |
| `query` | `string` | `undefined` | Optional channel holding query text for embedding-based re-scoring. |

### `mergeRanker` (RRF fusion)

Fuse several retrieval streams — e.g. a BM25 list and a semantic list — into one ranking with
**Reciprocal Rank Fusion**. Each item's contribution from a list is `1 / (rrfK + rank)` (rank
0-based); items are identified across lists by `idKey` and the fused `score` is the summed RRF
weight.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | — | Channels each holding a retrieval-result array to fuse. |
| `into` | `string` | — | Channel receiving the fused `{ id, content, score }` array. |
| `idKey` | `string` | `"id"` | Object field identifying items across lists. |
| `k` | `number` | keep all | Keep only the top-`k` fused results. |
| `rrfK` | `number` | `60` | Reciprocal Rank Fusion constant. |

### LLM reranking — External seam (deprecated TS)

The deprecated `@adriane-ai/rag-pipeline` package ships an `LLMReranker` that scores each result by
prompting the LLM Gateway. There is **no LLM reranker component in the Rust crate**; reranking that
ships as a graph component is the deterministic `reranker` above. Treat provider-backed reranking as
an external seam, routed through the gateway, not a built-in component.

## The RAG pipeline

A retrieval-QA flow chains these components with the rest of the catalog. A typical shape:

1. **Retrieve** — one or more retrievers (`bm25Retriever`, `keywordRetriever`, `retriever`) write
   candidate arrays to channels.
2. **Fuse / rerank** — `mergeRanker` fuses multiple retriever outputs (hybrid search), then
   `reranker` reorders the survivors.
3. **Assemble** — `answerBuilder` renders the top results as numbered citations and stitches the
   final answer (see the [component catalog](/docs/reference/component-catalog)).
4. **Govern** — a conditional edge inspects the answer and routes anything without a citation into a
   `humanGate` instead of publishing — the governance seam plain RAG stacks lack.

For an end-to-end, self-verifying program that wires retrieval → answer → citation check →
human-gate, follow the [RAG question answerer recipe](/docs/recipes/rag-question-answerer).

## Embeddings

Retrieval components never call a provider. The lexical retrievers need no vectors at all; `retriever`
uses a deterministic mock embedder; `semanticRetriever` consumes embeddings produced **upstream** by
the LLM Gateway's embeddings adapter (BYOM — real Mistral via `MISTRAL_API_KEY`, or a deterministic
offline hash when no key is present, so retrieval runs keyless in dev/test). See
[Vector stores](/docs/integrations/vector-stores/overview) for the storage seam and embeddings notes.

## See also

- [Component catalog](/docs/reference/component-catalog) — every retriever/reranker kind with its params.
- [RAG question answerer](/docs/recipes/rag-question-answerer) — end-to-end retrieval QA with a governance gate.
- [Vector stores](/docs/integrations/vector-stores/overview) — the `VectorStore` seam, the in-memory default, embeddings.
