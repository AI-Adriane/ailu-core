---
sidebar_position: 1
title: Vector stores
description: The VectorStore / BaseStore seams, the in-memory default, and durable storage (planned).
---

# Vector stores

Semantic retrieval in Ailu sits behind two engine seams. The engine ships **in-memory**
implementations so dev and test run full-fidelity with zero infrastructure; durable backends are
control-plane concerns implemented behind the same interfaces.

- **`VectorStore`** (`@ailu-ai/rag-pipeline`) — chunk upsert + cosine top-k for the RAG pipeline.
- **`BaseStore`** (`@ailu-ai/memory-store`) — namespaced key/value + semantic search for agent memory.

For the document/knowledge-graph model and its `KnowledgeStore` seam, see
[Knowledge base and graph](/docs/knowledge/knowledge-base-and-graph).

:::note Engine packages are deprecated as direct imports
Both `@ailu-ai/rag-pipeline` and `@ailu-ai/memory-store` are TypeScript fallbacks. The
authoritative implementations are the Rust `crates/rag-pipeline` and `crates/memory-store`, reached
through `@ailu-ai/napi` and consumed via `@ailu-ai/graph-sdk`. New code should build retrieval
and memory through `@ailu-ai/graph-sdk`, not by importing the engine packages directly. See ADR
0003 (TS engine deprecated, SDK on Rust).
:::

## The `VectorStore` seam (RAG)

The interface is two methods over `Chunk` / `RetrievalResult` (from `@ailu-ai/rag-pipeline`):

```ts
interface VectorStore {
  upsert(chunks: Chunk[]): Promise<void>;
  search(embedding: number[], topK: number): Promise<RetrievalResult[]>;
}
```

| Type | Shape |
| --- | --- |
| `Document` | `{ id, content, metadata, embedding? }` |
| `Chunk` | `Document & { sourceId, chunkIndex }` |
| `RetrievalResult` | `{ chunk, score }` |

### `InMemoryVectorStore`

The only implementation that ships. It keeps chunks in a `Map<id, Chunk>`, and `search` ranks every
stored chunk by **cosine similarity** against the query embedding, sorts descending, and slices the
top `topK`. Chunks with no `embedding` (or a zero vector) score `0`. It is O(n) per query — fine for
dev/test and small corpora, not for scale.

```ts
import { InMemoryVectorStore } from "@ailu-ai/graph-sdk";

const store = new InMemoryVectorStore();

await store.upsert([
  { id: "c1", sourceId: "doc-a", chunkIndex: 0, content: "…", metadata: {}, embedding: [0.1, 0.2, 0.3] }
]);

const hits = await store.search([0.1, 0.2, 0.25], 5);
// → RetrievalResult[] sorted by cosine score, highest first
```

## The `BaseStore` seam (agent memory)

Agent memory is a namespaced store with KV access, prefix listing, and semantic search
(`@ailu-ai/memory-store`):

```ts
interface BaseStore {
  get(namespace: MemoryNamespace, key: MemoryKey): Promise<MemoryItem | undefined>;
  put(namespace: MemoryNamespace, key: MemoryKey, value: unknown): Promise<MemoryItem>;
  delete(namespace: MemoryNamespace, key: MemoryKey): Promise<void>;
  search(namespace: MemoryNamespace, query: string, topK: number): Promise<MemoryItem[]>;
  list(namespace: MemoryNamespace, prefix?: string): Promise<MemoryItem[]>;
}
```

| Type | Shape |
| --- | --- |
| `MemoryNamespace` | `string[]` (hierarchical) |
| `MemoryKey` | `string` |
| `MemoryItem` | `{ namespace, key, value, createdAt, updatedAt, embedding? }` |

### `InMemoryStore`

The only working implementation. Items live in a `Map`, keyed by `namespace.join("|") + ":" + key`.
`put` stamps `createdAt`/`updatedAt` ISO timestamps (preserving the original `createdAt` on update).
`search` is **substring matching** today — it filters items in the namespace whose JSON-stringified
value or key contains the lowercased query, then slices `topK`. It is not vector search; the
`embedding` field exists on `MemoryItem` but is not used for ranking by this implementation.

```ts
import { InMemoryStore } from "@ailu-ai/graph-sdk";

const store = new InMemoryStore();

await store.put(["tenant-1", "agent-7"], "pref:tone", { tone: "formal" });
await store.search(["tenant-1", "agent-7"], "formal", 5); // substring match → MemoryItem[]
await store.list(["tenant-1", "agent-7"], "pref:");        // prefix filter
```

### `PgStore` — Planned (not implemented)

`PgStore` exists in `@ailu-ai/memory-store` but **every method throws**
`"…is not implemented yet."` (`search` throws `"PgStore.search with pgvector is not implemented yet."`).
It is a placeholder, not a usable backend. Do not wire it into anything.

```ts
// Current behavior — all methods throw:
await new PgStore().get(["ns"], "k"); // Error: PgStore.get is not implemented yet.
```

## Durable, vector-indexed storage — Planned

Durable semantic storage is **designed but not yet built**. The memory architecture ADR
(ADR 0026 — *Accepted*; the OSS in-memory seam shipped, durable persistence is the control-plane
piece below) supersedes the original pgvector plan: it resolves the
backend to **Neo4j 5** ("everything in Neo4j" — documents, entities, edges, and agent memory, with a
**native vector index** for semantic search). Postgres stays the relational control plane only
(tenancy, namespace ownership, runs, approvals, events); **pgvector is explicitly not used**.

Status of the moving parts, honest:

| Plane | Seam | In-memory default | Durable backend |
| --- | --- | --- | --- |
| RAG retrieval | `VectorStore` | `InMemoryVectorStore` (cosine, O(n)) | Planned (control plane, behind the seam) |
| Agent memory | `BaseStore` | `InMemoryStore` (substring search) | Planned — `PgStore` is a throwing stub; ADR 0026 targets Neo4j |
| Knowledge base | `KnowledgeStore` | `InMemoryKnowledgeStore` (cosine) | Built — Postgres-backed in the control plane (in-process cosine; no ANN index yet) |

Key points from ADR 0026, so the gap is clear:

- The **engine seams do not change** — `VectorStore` / `BaseStore` / `KnowledgeStore` stay DB-free with
  their in-memory defaults. Only the **control-plane** implementations behind them gain a durable
  backend. This is the open-core boundary: persistence is a control-plane concern.
- The chosen backend is **Neo4j Community, self-hosted** (sovereign, all deploy modes), with a native
  cosine vector index. External vector DBs (Pinecone / Weaviate / Qdrant) and pgvector remain
  available behind the seam but are not the default.
- An **external vector DB is a seam, not a built-in** — any durable backend is plugged in behind
  `VectorStore` / `BaseStore` by the control plane; the engine itself ships only the in-memory defaults.

Until ADR 0026 ships, durable agent memory does not exist: `InMemoryStore` is per-process and not
persisted across runs.

## Embeddings

Embedding vectors are produced through the LLM Gateway's embeddings adapter (BYOM) — real Mistral via
`MISTRAL_API_KEY`, or a deterministic offline hash when no key is present, so retrieval runs keyless in
dev/test. A `VectorStore` / `BaseStore` only stores and ranks vectors; it never calls a provider.

## See also

- [Knowledge base and graph](/docs/knowledge/knowledge-base-and-graph) — the `KnowledgeStore` seam, the
  document/graph model, and the built Postgres-backed control-plane store.
- [Models overview](/docs/integrations/models/overview) — providers and BYOM (embeddings included).
