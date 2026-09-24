---
sidebar_position: 1
title: Graphs, nodes, edges, state
description: The four building blocks of every Ailu graph.
---

# Graphs, nodes, edges, state

Every Ailu workflow is a **graph**: a set of **nodes** connected by **edges**, executing
over a shared, typed **state**. Four concepts, and everything else builds on them.

## State

State is a set of named **channels**. Each channel has a type, a default, and a **reducer**
that decides how a node's output is merged into it. The state type is inferred as you declare
channels, so `run()`'s result is fully typed.

```ts
createGraph({ name: "demo" })
  .channel("amount", { type: "number", default: 0 })       // replace (default)
  .channel("log", { type: "array", reducer: "append", default: [] })
```

Reducers are covered in depth in [Channels and reducers](./channels-and-reducers).

## Nodes

A node is a unit of work. The kinds you'll use:

| Node | What it is | Added with |
| --- | --- | --- |
| **Action** | A function `async (input, state, ctx) => update` returning a partial channel map. | `.node(id, handler)` |
| **Agent** | A ReAct loop over an LLM that writes its `AgentResult` into an output channel. | `.agentNode(id, config)` |
| **Tool node** | Executes tool calls produced upstream. | `.toolNode(...)` |
| **Human gate** | Suspends the run for a human decision, then resumes. | `.humanGate(id)` |

Whatever the kind, a node's contract is the same: read the typed state, do work, return an
update. The runtime checkpoints after the node completes.

## Edges

Edges decide control flow:

- **`.edge(from, to)`** — always followed.
- **`.conditionalEdge(from, to, name, predicate)`** — followed only when the **named
  predicate** holds. Predicates are registered functions over the live state, **never
  `eval`'d strings** — that is what keeps a graph safe to inspect and replay.
- **Fan-out (`send`)** — a node can emit multiple work items that run as parallel branches.

```ts
.conditionalEdge("review", "publish", "approved", (s) => s.channels.decision === "approve")
```

## The graph

`createGraph({ name })` returns a builder; `.compile()` validates it and returns a runnable
`CompiledGraph`. Compilation enforces the structural rules (no dangling edges, a reachable
entry, unique node ids); a failure is a typed `GraphCompileError`, or a `Result` if you use
`.safeCompile()`.

```ts
const app = createGraph({ name: "order" })
  .node("validate", async () => ({}))
  .node("charge", async () => ({}))
  .edge("validate", "charge")
  .compile();

const out = await app.run({});
```

## How a run executes

The runtime walks the graph from the entry node, executing one node at a time and following
edges whose predicates hold. After **every** node it writes a checkpoint and emits lifecycle
events. When it reaches a human gate it suspends; a later resume continues from the checkpoint.
That contract — checkpoint-after-every-node, event-per-transition — is the subject of the
[execution contract](./execution-contract).

## Next

- [Channels and reducers](./channels-and-reducers)
- [The execution contract](./execution-contract)
