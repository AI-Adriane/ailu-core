---
sidebar_position: 3
title: Resume across processes
description: Suspend a governed run in one process, persist the checkpoint, and resume it in another — by implementing the Checkpointer interface or running on Ailu Studio.
tags: ["state", "ops"]
difficulty: advanced
---

# Resume across processes

The realistic shape of a human-approval workflow: one process starts a run, it **suspends**
for approval, the process exits, and hours later a *different* process resumes it. To cross that
process boundary you need two things — a **stable run id** and a **persisted checkpoint** — and a
resume entry point that takes the checkpoint as data rather than reading it from in-process
memory.

The engine is a **library you embed**, not a server. It ships the `Checkpointer` **interface**
plus an `InMemoryCheckpointer` (great for a single process, tests, and the dev loop). In-memory
checkpoints die with the process, so to resume across a boundary you have two honest options:

1. **Implement the `Checkpointer` interface** against a durable store you own (Postgres, Redis,
   S3, a file) and wire it into your own service. The engine never embeds a DB schema — the
   persistence is yours.
2. **Use [Ailu Studio](/docs/roadmap)** — the managed control plane — which provides durable
   checkpointing, a worker fleet, and the governance UI out of the box, so you don't build or
   operate any of it.

This recipe shows option 1 end to end, then points at the seam Studio builds on.

```mermaid
sequenceDiagram
  participant P1 as Process A
  participant Store as Your store (Postgres, …)
  participant P2 as Process B
  P1->>P1: runCatalogGraph(def, { runId })
  P1->>Store: persist outcome.state (status "suspended")
  Note over P1: process A can exit
  P2->>Store: load the persisted GraphState
  P2->>P2: resumeCatalogGraph(def, state, { approvedTools })
  P2-->>P2: status "completed"
```

## The serializable seam

The catalog run path — `runCatalogGraph` and `resumeCatalogGraph` — runs a plain
`GraphDefinition` on the Rust engine and returns a **serializable `GraphState`** you can store
anywhere and hand back later. Because the checkpoint comes back to you *as data*, you decide
where it lives. This is the seam a control plane (your own, or Ailu Studio) builds durable
resume on top of.

### Process A — start and persist

`runCatalogGraph(definition, options)` runs to completion or suspension and returns a
`CatalogRunOutcome`: `{ state, status, usedRustEngine }`. The `state` is a full `GraphState`
(channels included) you can `JSON.stringify` and persist.

```ts
import {
  runCatalogGraph,
  docQaReferenceDefinition, // any carrier-bearing GraphDefinition works
  type RunId
} from "@ailu-ai/graph-sdk";

const definition = docQaReferenceDefinition();
const RUN_ID = "run_refund_42" as RunId;

const outcome = await runCatalogGraph(definition, {
  runId: RUN_ID,
  initialData: { question: "…" }
});

console.log(outcome.status); // "suspended" when it parked for approval

if (outcome.status === "suspended") {
  // Persist the checkpoint anywhere durable — keyed by the stable run id.
  await myStore.put(RUN_ID, JSON.stringify(outcome.state));
}
// Process A can now exit. The checkpoint outlives it.
```

**Expected result:** `outcome.status` is `"suspended"` for a graph that hits a gate, and
`outcome.state` is a JSON-serializable snapshot you control the persistence of.

### Process B — load and resume

A fresh process loads the persisted state and calls `resumeCatalogGraph(definition, state, …)`.
The definition must be the *same* graph (it is data — store it, or rebuild it from the same
source). For a governed resume, pass the human-approved tools with their provenance.

```ts
import { resumeCatalogGraph, type GraphState, type RunId } from "@ailu-ai/graph-sdk";

const RUN_ID = "run_refund_42" as RunId;
const definition = docQaReferenceDefinition(); // the same graph, rebuilt or loaded
const state = JSON.parse(await myStore.get(RUN_ID)) as GraphState;

const resumed = await resumeCatalogGraph(definition, state, {
  // Each granted tool carries who requested it and who resolved it. The engine
  // re-validates the no-self-approval invariant per tool before unlocking it.
  approvedTools: [
    { name: "refund", requestedBy: "assistant", resolvedBy: "ops-lead@acme.com" }
  ]
});

console.log(resumed.status); // "completed"
```

**Expected result:** `resumed.status` is `"completed"`, and the gated tool executed exactly once
— in process B, after the approval, never in process A.

For an *ungoverned* resume (no gated tools to unlock), omit `approvedTools`:

```ts
const resumed = await resumeCatalogGraph(definition, state);
```

:::warning The Rust engine is required for the catalog seam
`runCatalogGraph` / `resumeCatalogGraph` throw `RustEngineUnavailableError` when the native
addon (`@ailu-ai/napi`) is absent — there is no TypeScript fallback for this seam. The Rust
engine re-validates the no-self-approval provenance on every resume (defence in depth). Both the
catalog path and `@ailu-ai/napi` ship in the open SDK. (Source:
`packages/graph-sdk/src/run-catalog-graph.ts`.)
:::

## Implementing a durable `Checkpointer`

The engine exports the `Checkpointer` interface and the in-memory implementation; persisting to a
real store is just implementing the same four methods against it. Here is a minimal sketch
against any key/value-ish store — adapt the body to Postgres, Redis, S3, or a file:

```ts
import { InMemoryCheckpointer } from "@ailu-ai/graph-sdk";
import type { Checkpointer, Checkpoint, CheckpointId } from "@ailu-ai/graph-runtime";
import type { RunId } from "@ailu-ai/graph-sdk";

// `InMemoryCheckpointer` is what the engine uses by default — swap in your own.
export class MyStoreCheckpointer implements Checkpointer {
  constructor(private readonly store: MyDurableStore) {}

  // Called after every node completion / state mutation — append, don't overwrite.
  async save(checkpoint: Checkpoint): Promise<void> {
    await this.store.put(`cp:${checkpoint.id}`, JSON.stringify(checkpoint));
    await this.store.put(`latest:${checkpoint.runId}`, checkpoint.id);
    await this.store.append(`run:${checkpoint.runId}`, checkpoint.id);
  }

  // The latest checkpoint for a run — what resume reads.
  async load(runId: RunId): Promise<Checkpoint | undefined> {
    const id = await this.store.get(`latest:${runId}`);
    return id ? this.loadById(id as CheckpointId) : undefined;
  }

  async loadById(id: CheckpointId): Promise<Checkpoint | undefined> {
    const raw = await this.store.get(`cp:${id}`);
    return raw ? (JSON.parse(raw) as Checkpoint) : undefined;
  }

  // Every checkpoint for a run, oldest first — powers time-travel / audit.
  async list(runId: RunId): Promise<Checkpoint[]> {
    const ids = await this.store.listAppended(`run:${runId}`);
    const all = await Promise.all(ids.map((id) => this.loadById(id as CheckpointId)));
    return all.filter((cp): cp is Checkpoint => cp !== undefined);
  }
}
```

A `Checkpoint` is `{ id, runId, graphState, createdAt }` — `graphState` is the same
JSON-serializable `GraphState` the catalog seam hands you, so the store only ever sees plain
JSON. Once you have a durable `Checkpointer`, your service persists on `save` and rehydrates on
`load` across any process boundary, with `list` backing time-travel and audit.

:::note Don't want to build and operate this?
[Ailu Studio](/docs/roadmap) — the managed control plane — provides durable checkpointing, a
worker fleet that picks up suspended runs, and a governance UI to review and approve them, so you
don't implement a `Checkpointer`, stand up a store, or run a worker yourself. The engine in this
repo gives you the `Checkpointer` interface and `InMemoryCheckpointer`; Studio is the platform
that runs durably on top of the same seam.
:::

## Why not `CompiledGraph.resume()` across processes?

`CompiledGraph` (from `createGraph(...).compile()`) keeps its suspended state **in memory on the
instance**. On the Rust engine, `resume` / `approveAndResume` must follow the suspension on the
*same* `CompiledGraph` instance — resuming on a fresh instance throws *"No suspended state for
run …"*. That makes `CompiledGraph` the right tool for a **single-process** suspend/resume (the
[refund agent](./governed-refund-agent) and [RAG](./rag-question-answerer) recipes), but **not**
for crossing a process boundary. (Source: `CompiledGraph.requireSuspendedState`,
`packages/graph-sdk/src/compiled-graph.ts`.)

For the cross-process case, use the catalog path (the checkpoint is **returned to you as data**)
or a durable `Checkpointer` of your own — or let Ailu Studio do it for you.

## Approvals across the boundary

The two halves connect through an `ApprovalEngine`. When `runCatalogGraph` is given an
`approvalEngine`, the moment the run suspends it files one request per gated tool
(`requestedBy = nodeId`) and stashes the engine ids in the `__approvalIds` channel of the
returned state. A human resolves those requests out of band (the engine forbids self-approval),
and process B resumes with only the engine-approved tools.

```ts
const outcome = await runCatalogGraph(definition, {
  runId: RUN_ID,
  initialData: { question: "…" },
  approvalEngine: myEngine // requests filed on suspension; ids in __approvalIds
});
```

## Related

- [Governed refund agent](./governed-refund-agent) — the single-process version of the loop.
- [Resumability and approvals](/docs/core-concepts/resumability-and-approvals) — the contract that makes resume exact.
- [Governance model](/docs/governance/governance-model) — request → approve → attest → resume.
- [The napi bridge](/docs/architecture/napi-bridge) — how the catalog seam reaches the Rust engine.
