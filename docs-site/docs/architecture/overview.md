---
sidebar_position: 1
title: Overview
description: The layered engine, the runtime contract, and the extension points.
---

# Architecture overview

Ailu is a **deterministic, resumable, observable** agentic-graph runtime. The engine is
written in **Rust** (the production path), with a **TypeScript SDK** as the public front door
(`@ailu-ai/graph-sdk`) and an equivalent TypeScript runtime as a **fallback** when the native
addon is absent.

## The engine layers

The engine follows a **one-directional dependency rule**:
`graph-core` (zero dependencies) → `graph-runtime` → `graph-sdk` (public API). `graph-core`
depends on no other internal package; any reverse import would break the foundation contract.

```text
graph-core      pure data model — zero side effects, zero internal dependencies
    │
graph-runtime   execution engine — NodeRegistry, ConditionRegistry, Checkpointer, EventBus
    │
graph-sdk       public fluent API — createGraph → CompiledGraph
```

### `graph-core` — the pure data model

Pure TypeScript, **no I/O** (no DB, HTTP, LLM, framework). It defines the founding types — all
branded (`NodeId`, `EdgeId`, `GraphId`, `RunId`) — plus schemas and errors.

| Type | Shape |
| --- | --- |
| `GraphDefinition` | `{ id, version, name, recursionLimit?, channels, nodes[], edges[], entryNodeId, metadata? }` |
| `GraphState` | `{ runId, graphId, currentNodeId, status, channels, version, checkpointId?, … }` |
| `NodeDefinition` | `{ id, type, label, subgraphId?, inputMapping?, outputMapping?, fanOut?, retryPolicy?, … }` |
| `EdgeDefinition` | `{ id, from, to, type, condition? }` |
| `Command` | `{ goto: NodeId \| NodeId[], update? }` |

Enumerations: **node types** (`action`, `agent`, `tool`, `human-gate`, `subgraph`), **edge
types** (`default`, `conditional`), **run statuses** (`idle`, `running`, `suspended`,
`completed`, `failed`), and **channel reducers** (`replace`, `append`, `merge`).

### `graph-runtime` — the execution engine

Built over a validated `GraphDefinition`. Four core abstractions — the runtime's **extension
points**:

| Abstraction | Role | Default |
| --- | --- | --- |
| `NodeRegistry` | map a node to its handler | `InMemoryNodeRegistry` |
| `ConditionRegistry` | map a condition **name** to a pure predicate | `InMemoryConditionRegistry` |
| `Checkpointer` | `save` / `load(runId)` / `loadById(id)` / `list(runId)` | `InMemoryCheckpointer` |
| `EventBus` | `emit(event)` / `subscribe(handler) → unsubscribe` | `InMemoryEventBus` |

A node handler returns either a **channel update** (`Partial<ResolvedChannels>`) or a
`Command { goto, update? }` that overrides default edge resolution. A condition predicate is
**synchronous and pure**: `(state) => boolean`.

:::tip Security invariant
Conditions are **always names** resolved in the `ConditionRegistry` — never `eval`'d code or
inline JS expressions. This is non-negotiable (inspectability + safety).
:::

### `graph-sdk` — the public API

`createGraph(options)` returns a fluent `GraphBuilder<TState>` that accumulates channels, nodes,
edges, handlers and conditions; `.compile()` produces a `CompiledGraph<TState>` with full type
inference. The compiled graph exposes `run`, `resume`, `approveAndResume`, `stream`, `onEvent`,
and `usesRustEngine`. See the [TypeScript SDK](/docs/sdk-parity/typescript-sdk).

## The runtime contract

Four guarantees, verified in the TS runtime and replicated in Rust. They are covered in depth in
[the execution contract](/docs/core-concepts/execution-contract):

1. **Checkpoint after every node** — a checkpoint is persisted after *every* node execution, not
   only on state mutation. This is the basis of resumability.
2. **An event per lifecycle transition** — the `RunEvent` union (`node_started`,
   `node_completed`, `node_failed`, `run_suspended`, `run_resumed`, `run_completed`,
   `run_failed`).
3. **Deterministic suspend / resume** — `human-gate` nodes suspend cleanly; resume loads the
   latest checkpoint, advances past the gate, emits `run_resumed`, and continues.
4. **A recursion limit** — `recursionLimit` bounds steps per run; exceeding it raises
   `RecursionLimitError` during the run.

**Resume robustness:** a reloaded checkpoint is re-validated with Zod (`parseGraphState`) — a
guard against corrupted or tampered state — and state equality uses a cycle-safe
`structuralEqual`, not `JSON.stringify`.

:::note Reserved, not implemented
Parallel fan-out (`NodeDefinition.fanOut`) and subgraphs (`NodeDefinition.subgraphId`) have slots
in the schema but are not implemented in the runtime yet. Don't rely on them until marked stable.
:::

## See also

- [The native bridge](./napi-bridge) — how Rust and TypeScript meet.
- [The execution contract](/docs/core-concepts/execution-contract)
- [Runtime and engine](/docs/core-concepts/runtime-and-engine)
