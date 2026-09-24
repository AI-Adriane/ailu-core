---
sidebar_position: 2
title: Knowledge base and graph
description: The KB/KG model, pure graph operations, and the KnowledgeStore seam.
---

# Knowledge base and graph

`@ailu/knowledge` (and its Rust twin `ailu-knowledge`) is the engine-owned model
for a **knowledge base** (documents + embeddings) and the **knowledge graph** over it
(documents as nodes, typed relations as edges). It ships the data shapes, the pure graph
operations, and a `KnowledgeStore` seam with an in-memory implementation.

The control plane implements a Postgres-backed `KnowledgeStore` and owns persistence,
tenancy, connectors and outbound activation — reusing the same model and operations.

## Model

| Type | What it is |
| --- | --- |
| `KbDocument` | a stored document (id, namespace, content, OKF fields, `createdAt`) |
| `KbGraphNode` | a document projected for graph views (`id`, `type`, `title?`) |
| `KbGraphEdge` | a typed edge (`from --type--> to`) |
| `KbGraph` | `{ nodes, edges }` for a namespace |
| `KbSearchHit` | a semantic-search hit (`id`, `content`, cosine `score`, …) |

## Pure operations

These are framework-free and deterministic:

```ts
import { resolveTargetId, buildEdges, buildGraph, neighbors, cosineSimilarity } from "@ailu/knowledge";

resolveTargetId("kb", "/a.md");                 // "kb:a.md"  — OKF target → entity id
buildEdges("kb", "kb:src.md", links, relations); // OKF links (→ "references") + typed relations, deduped
buildGraph(docs, edges);                          // documents → nodes + the edges
neighbors(graph, "kb:a.md", 2);                   // depth-limited subgraph (BFS over outgoing edges)
cosineSimilarity(a, b);                            // ranking primitive
```

## The KnowledgeStore seam

```ts
interface KnowledgeStore {
  putDocument(document, embedding): Promise<void>;
  getDocument(namespace, id): Promise<KbDocument | undefined>;
  listDocuments(namespace): Promise<KbDocument[]>;
  setRelations(namespace, fromId, edges): Promise<void>;
  listRelations(namespace): Promise<KbGraphEdge[]>;
  search(namespace, queryEmbedding, k): Promise<KbSearchHit[]>;
  graph(namespace): Promise<KbGraph>;            // default: built from the ops above
  neighbors(namespace, id, depth): Promise<KbGraph>;
}
```

`InMemoryKnowledgeStore` implements it with cosine search + graph traversal — the
standalone/dev/test backend:

```ts
import { InMemoryKnowledgeStore, buildEdges } from "@ailu/knowledge";

const store = new InMemoryKnowledgeStore();
await store.putDocument(doc, embedding);
await store.setRelations("kb", "kb:a.md", buildEdges("kb", "kb:a.md", ["/b.md"], []));
const hits = await store.search("kb", queryVector, 5);
const graph = await store.graph("kb");
```

`graph` and `neighbors` are default methods built on `listDocuments` + `listRelations` and
the pure ops, so a backend only has to provide storage.
