---
sidebar_position: 6
title: Memory architecture
description: The four memory planes — execution, knowledge, agent memory, and portable export — behind one governed seam, with deterministic, provenance-tagged recall.
---

# Memory architecture

"Memory" in Ailu is not one thing. It is four distinct planes that grew for different
reasons and have different lifecycles (ADR 0026). Naming them keeps the picture honest — only
two of the four are about *recall across runs*, and only one is the institutional knowledge moat.

| Plane | What it is | Lifetime | Where it lives |
| --- | --- | --- | --- |
| **M1 Execution** | Run state channels + checkpoints — the determinism/resume spine. | One run (resumable). | `graph-runtime` (Rust, authoritative). |
| **M2 Knowledge** | Institutional memory: documents + a typed entity graph. Tenant-scoped, role-gated, shared. | Durable, org-owned. | `KnowledgeStore` seam → control plane. |
| **M3 Agent memory** | Per-agent / per-tenant recall across runs (vector + graph). | Durable, per agent. | `MemoryStore` seam → control plane. |
| **M4 Portable** | Versioned export/import of a namespace's memory (BKP, built on OKF). | A bundle. | `okf` + control plane. |

M1 is covered by the [execution contract](./execution-contract) and
[resumability](./resumability-and-approvals) — it is run-scoped and out of scope here. This page is
about the **long-term** planes (M2, M3) and the one model that unifies how they are written and read.

## One governed seam, two recall modalities

M3 agent memory is served by a single DB-free seam, `MemoryStore`, that offers **both** recall
modalities the thesis needs:

- **Vector** (semantic) — the items in a namespace nearest to a query embedding, by cosine.
- **Graph** — the entities within *n* hops of a seed entity, over typed edges.

`RecallMode` selects which run: `"vector"`, `"graph"`, or `"both"` (the default). The seam works over
four value types: a recallable **`MemoryItem`** (the vector unit), a first-class **`MemoryEntity`**
(person / project / decision / system / policy — an open kebab vocabulary), a typed **`MemoryEdge`**
between entities, and a **`MemoryProvenance`** stamped on every write. Retrieval is bounded and
ordered by a pure **`RetrievalPolicy`** (`{ topK, mode }`).

These are the **Rust engine** types (`crates/memory`). You do not construct them directly from the
TypeScript or Python SDK — you reach them through the `memory` overlay on an agent node (below), and
the engine threads them through for you.

## OSS default: in-memory, zero infrastructure

The engine ships exactly one implementation, `InMemoryMemoryStore`: cosine vector recall plus a
depth-limited adjacency-BFS graph recall, kept in process, **zero key and zero DB**. The OSS dev
experience is full-fidelity — recall works across runs *within a process* with no setup.

Durable, cross-process persistence is a **control-plane** concern behind the same seam. ADR 0026
resolves the backend to **Neo4j Community, self-hosted** ("everything in Neo4j": M2 documents +
entities + edges and M3 agent memory, with a native vector index for semantic search). The engine
seam shape does not change — only the implementation plugged in behind it does. This is the open-core
boundary: persistence and governed LLM entity extraction live in the control plane; the engine stays
DB-free. `neo4j-driver` never enters an engine crate. (For the storage-seam status across RAG,
agent memory, and the knowledge base, see [Vector stores](/docs/integrations/vector-stores/overview).)

## How an agent uses memory

You opt an agent into M3 with one overlay. Before the run the engine recalls relevant past context
and injects it into the seed; after the run it persists what the agent learned, attributed. It is
the [`MemoryMiddleware`](/docs/advanced-agents/middleware-and-profiles) — recall-before / persist-after.

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "assistant" })
  .agentNode("reply", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Answer the user." },
    memory: { namespace: "tenant:acme:agent:assistant", topK: 5, recall: "vector" }
  })
  .compile();

// First run learns; a later run recalls it into context automatically.
await app.run({ question: "our deploy window is Tuesdays 9-11am" });
await app.run({ question: "when can I deploy?" }); // recalls the earlier fact
```

This runs on the Rust engine: the overlay threads SDK → wire → bridge → the governed middleware
stack, and the run completes on the native path (it is **not** a TypeScript-only convenience).

| `memory` field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `namespace` | `string` | — (required) | Tenant-scoped memory partition (e.g. `tenant:{tenant}:agent:{id}`). **Sealed by the engine** with the principal — never user-routable. |
| `topK` | `number` | `5` | How many memories to recall. |
| `recall` | `"vector"` \| `"graph"` \| `"both"` | `"both"` | Which recall modality runs. Graph-only auto-recall needs entity linking (control-plane extraction); meanwhile the seam's graph recall is available to callers directly. |

## Governed by construction

Memory is the same bet as the council and the filesystem: not a new trick, the **governed** version
of a known primitive.

- **Provenance on every write.** `MemoryProvenance` records who / what / when / from-which-source /
  with-what-confidence / under-which-status (`asserted` → `verified` → `rejected`). Empty fields stay
  off the wire, so a minimal provenance is cheap. A governed run can answer *"what knowledge informed
  this action."*
- **Tenant-scoped at the seam.** The `namespace` and the `principal` are sealed at construction by
  the bridge — user data never supplies them — so recall is tenant-scoped by construction, not by a
  runtime check that could be skipped. A namespace you cannot access returns an empty result, never
  disclosure.
- **Deterministic recall.** Vector recall is score-desc with an explicit **insertion-order tie-break**;
  graph recall is deterministic discovery order. A governed run re-recalls the same set on resume —
  reads obey the determinism contract, not just writes.
- **No new runtime path.** Recall only mutates the **seed** conversation (no state change, no new
  checkpoint kind), so M1's checkpoint/resume guarantees are inherited. Recall and persist are
  **fail-open**: a memory error never sinks an otherwise-good run.

## Scope today vs deferred

The OSS engine ships vector recall + provenance-tagged persist (heuristic — it stores the run's
reasoning as a recallable item). **No LLM claim-writing** happens in the engine: governed LLM entity
extraction — reading a corpus and asserting typed entities/edges about people and decisions — is a
control-plane concern (ADR 0026 §2), the riskiest plane and the one that gets the closest review. The
entity-graph seam (`put_entity` / `put_edge` / `neighbors`) exists and is exercised, ready for that
control-plane extractor to write into.

## See also

- [Long-term agent memory (recipe)](/docs/recipes/agent-memory) — the end-to-end recall-across-runs walkthrough.
- [Vector stores](/docs/integrations/vector-stores/overview) — the storage-seam status (RAG, agent memory, KB) and the in-memory defaults.
- [Knowledge base and graph](/docs/knowledge/knowledge-base-and-graph) — the M2 `KnowledgeStore` seam and the document/entity graph.
- [Middleware and profiles](/docs/advanced-agents/middleware-and-profiles) — where the recall-before / persist-after middleware installs.
- [Execution contract](./execution-contract) — M1, the run-scoped plane this page builds on.