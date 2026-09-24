---
sidebar_position: 1
title: Checkpointers overview
description: Bring your own store — the Checkpointer interface plus an in-memory default; durable cross-process checkpointing and a Postgres checkpointer ship with Ailu Studio, not the open engine.
---

# Checkpointers overview

A **checkpointer** is where the runtime persists `GraphState`. The contract is fixed: the engine
checkpoints after **every** node completion and state mutation, so any suspended run can be
resumed from its latest checkpoint. *Where* those checkpoints live is yours to decide — the engine
is a library you embed, not a server with a baked-in database.

The open SDK ships the `Checkpointer` **interface** plus exactly one concrete implementation,
`InMemoryCheckpointer` (the default). Durable, cross-process checkpointing — and a Postgres
checkpointer — are provided by **Ailu Studio**. The open engine does **not** ship a Postgres
checkpointer.

## The default (zero config)

Every compiled graph gets an `InMemoryCheckpointer` unless you pass your own. No setup, no
environment variables.

```ts
import { createGraph, InMemoryCheckpointer } from "@ailu/graph-sdk";

createGraph({ name: "pipeline" })
  // .checkpointer(new InMemoryCheckpointer()) — implicit; this is the default
  .node("step", async () => ({}))
  .compile();
```

See [`.checkpointer(cp)`](/docs/reference/builder-api#checkpointercp) for the builder method.

## The `Checkpointer` interface

Four async methods. Implement them against any durable store to persist runs across process
boundaries. Exported from `@ailu/graph-runtime`.

```ts
import type { Checkpointer, Checkpoint, CheckpointId } from "@ailu/graph-runtime";
import type { RunId } from "@ailu/graph-sdk";

interface Checkpointer {
  save(checkpoint: Checkpoint): Promise<void>;
  load(runId: RunId): Promise<Checkpoint | undefined>;
  loadById(id: CheckpointId): Promise<Checkpoint | undefined>;
  list(runId: RunId): Promise<Checkpoint[]>;
}
```

| Method | Called when | Must return |
| --- | --- | --- |
| `save(checkpoint)` | after every node completion / state mutation | resolves once persisted (append, don't overwrite) |
| `load(runId)` | on resume | the **latest** checkpoint for the run, or `undefined` |
| `loadById(id)` | on time-travel / targeted replay | the exact checkpoint, or `undefined` |
| `list(runId)` | for audit / time-travel | every checkpoint for the run |

A `Checkpoint` is plain, JSON-serializable data:

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `CheckpointId` | branded `string`; unique per checkpoint |
| `runId` | `RunId` | the run this snapshot belongs to |
| `graphState` | `GraphState` | full channel state — what resume rehydrates |
| `createdAt` | `string` | ISO timestamp |

Because the snapshot is plain JSON, your store only ever sees serializable data — implementing a
Postgres, Redis, S3, or file-backed checkpointer is just persisting these four methods. Full
walkthrough: [Resume across processes](/docs/recipes/resume-across-processes#implementing-a-durable-checkpointer).

## `InMemoryCheckpointer` (default)

Process-local. Holds every checkpoint in `Map`s keyed by `CheckpointId` and `RunId`. Ideal for
development, tests, and single-process runs.

| Property | Value |
| --- | --- |
| Storage | in-process `Map` (heap) |
| Scope | single process |
| Durability | **none** — checkpoints die with the process |
| Cross-process resume | not possible |
| Config / env vars | none |

In-memory checkpoints cannot cross a process boundary: the suspend-here / resume-there shape of a
human-approval workflow needs a durable store. That is the next section.

## Durable & cross-process: bring your own, or use Studio

Two honest options for persisting runs beyond a single process:

| Option | What you build | What you operate |
| --- | --- | --- |
| **Implement `Checkpointer`** | the four methods against your store (Postgres / Redis / S3 / file) | your store, your service, your worker |
| **Ailu Studio** | nothing | nothing — managed |

[**Ailu Studio**](/docs/roadmap) — the managed control plane — provides durable checkpointing,
a **Postgres checkpointer**, a worker fleet that picks up suspended runs, and a governance UI to
review and approve them. The open engine in this repo gives you the seam (`Checkpointer` interface
+ `InMemoryCheckpointer`); Studio is the platform that runs durably on top of the same seam.

:::note The open engine ships no Postgres checkpointer
A Postgres-backed `Checkpointer` is **not** part of the open SDK. Either implement the interface
yourself or run on Ailu Studio. Studio's persistence uses `DATABASE_URL`; the open engine reads
no checkpointer-specific environment variables.
:::

## Related

- [Builder API · `.checkpointer(cp)`](/docs/reference/builder-api#checkpointercp) — wiring a checkpointer into a compiled graph.
- [Resume across processes](/docs/recipes/resume-across-processes) — implementing a durable `Checkpointer` end to end.
- [Roadmap / Ailu Studio](/docs/roadmap) — the managed control plane.
