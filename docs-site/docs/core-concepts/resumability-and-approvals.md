---
sidebar_position: 4
title: Resumability and approvals
description: How a run suspends at a human gate and resumes exactly from the latest checkpoint.
---

# Resumability and approvals

Because Ailu checkpoints after every node, a run can **stop and continue later** without
losing or repeating work. The headline use of that capability is the **human gate**: a node
that suspends the run for a human decision.

## Suspend and resume

```mermaid
sequenceDiagram
  participant Agent
  participant Runtime
  participant Checkpoint
  participant Human
  
  Agent->>Runtime: run()
  Note over Runtime: execute write node
  Runtime->>Runtime: reach humanGate
  Runtime->>Checkpoint: persist state at review
  Runtime->>Agent: emit run_suspended
  Agent->>Agent: process exits
  
  Human->>Runtime: resume(runId)
  Runtime->>Checkpoint: load latest checkpoint
  Checkpoint->>Runtime: state restored at review
  Runtime->>Agent: emit run_resumed
  Note over Runtime: continue from review
  Runtime->>Runtime: execute publish node
  Runtime->>Agent: emit run_completed
```

*Suspend → Approve → Resume lifecycle: a run checkpoints at humanGate, suspends cleanly, and resumes from the latest checkpoint after human decision.*

A `humanGate` node suspends the run cleanly. The run's `status` becomes `"suspended"` and
`currentNodeId` points at the gate. The process can exit entirely — the state is checkpointed.

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "…" }))
  .humanGate("review")
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const first = await app.run({});
console.log(first.status);        // "suspended"
console.log(first.currentNodeId); // "review"
```

Later — same process or a fresh one, as long as the checkpoint is durable — you resume:

```ts
const resumed = await app.resume(first.runId);
console.log(resumed.status);      // "completed"
```

The runtime emits `run_suspended` when it parks at the gate and `run_resumed` when it picks
back up, then continues `publish → run_completed`. Observers replaying the event journal see
exactly that sequence.

## Durable checkpoints

The engine ships the `Checkpointer` **interface** plus an `InMemoryCheckpointer`. In-memory is
fine for tests and single-process runs. To suspend in one process and resume in another — the
realistic shape of a human-approval workflow — you need durable checkpoints. You have two paths:

**Implement the interface against your own store.** The engine never assumes a particular
backend; you wire `Checkpointer` to Postgres, Redis, or anything else, and pass your instance
to `.checkpointer(...)`:

```ts
import { createGraph, type Checkpointer } from "@ailu-ai/graph-sdk";

// Your own implementation, backed by whatever store you run.
const myCheckpointer: Checkpointer = createMyCheckpointer({
  connectionString: process.env.DATABASE_URL
});

const app = createGraph({ name: "publish-flow" })
  .checkpointer(myCheckpointer)
  // …nodes…
  .compile();
```

Once the checkpoints are durable, `app.resume(runId)` works across process boundaries: one
process suspends a run, a human approves hours later, and a fresh process resumes it from the
persisted checkpoint.

:::tip Don't want to build the durable layer?
Use **Ailu Studio**, the managed control plane — it provides durable checkpointing, a worker
fleet, and the governance UI out of the box, so you embed the open engine in your app and let
Studio persist and resume runs for you. See the [Governance](/docs/governance/governance-model)
section.
:::

## Approvals are governed resumes

A human gate is the mechanism; **governance** is the policy layered on top. A gate can require
an explicit *approval*: a decision recorded by an authenticated principal who is **not** the
agent that requested it, attested and audited. That full loop — request, approve/reject,
attest, resume — is the subject of the [Governance](/docs/governance/governance-model) section.

## Next

- [Runtime and engine](./runtime-and-engine)
- [Approval gates](/docs/governance/approval-gates)
