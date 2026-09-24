---
sidebar_position: 3
title: The execution contract
description: Determinism, checkpoint-after-every-node, an event per transition, and the recursion limit.
---

# The execution contract

Ailu's runtime makes four promises. They are not optional optimisations — they are the
contract that makes a run **resumable, observable, and safe to replay**. Everything else in
the framework depends on them holding.

## 1. Deterministic by default

The same graph and the same inputs produce the same execution. Two design choices enforce this:

- **Conditions are named predicates, never `eval`'d strings.** A conditional edge references a
  predicate you registered by name; the engine never compiles or evaluates a user-supplied
  string. A graph's routing is therefore fully inspectable and can't smuggle in side effects.
- **State merges are reducer-declared**, not decided inside handlers (see
  [channels and reducers](./channels-and-reducers)).

Non-determinism (an LLM call, a clock, a random value) is confined to node handlers and
captured in the checkpointed state — so a *resume* replays the recorded outcome, not a fresh
roll of the dice.

## 2. Checkpoint after every node completion and state mutation

The runtime writes a checkpoint after **every** node finishes and every state mutation. A
checkpoint is the full, typed state plus the position in the graph. This is what makes resume
exact: a crashed or suspended run continues from the latest checkpoint with **no completed work
re-run** — and, crucially, no completed *side effects* (a charge, an email) repeated.

Checkpointers are pluggable. The engine ships the `Checkpointer` interface plus an
`InMemoryCheckpointer` for tests and single-process runs; for durable, cross-process
resumption you implement the interface against your own store (Postgres, Redis, …) — or use
**Ailu Studio**, the managed control plane that provides durable checkpointing for you. See
[durable checkpoints](/docs/core-concepts/resumability-and-approvals).

## 3. An event for every node lifecycle transition

Every transition — node started, node completed, run suspended, run resumed, run completed,
run failed — emits an event onto the event bus. The **event journal is the audit trail**, and
it is what a live run view replays to stay in sync (see
[observable runs](/docs/governance/observable-runs)). If a transition happened, there is an
event for it; if there is no event, it did not happen.

## 4. A recursion limit

Cyclic graphs are allowed (an agent can loop), so the runtime enforces a `recursionLimit` to
bound execution. Exceeding it stops the run with a typed error rather than spinning forever.
Set it per graph via `createGraph({ recursionLimit })`.

## Why this matters

Drop any one of these and the guarantees collapse:

- No determinism → replay diverges.
- No checkpointing → a crash re-runs side effects.
- No events → no audit trail, no live view.
- No recursion limit → a cyclic agent never terminates.

When you write a custom node handler, you are operating *inside* this contract: do your work,
return an update, and let the runtime checkpoint and emit. Don't reach around it.

## Next

- [Resumability and approvals](./resumability-and-approvals)
- [Runtime and engine](./runtime-and-engine)
