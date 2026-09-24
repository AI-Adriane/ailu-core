---
sidebar_position: 10
title: Subgraphs
description: Compose graphs by nesting one graph as a single node inside another.
---

# Subgraphs

A **subgraph node** runs another compiled graph as a single step of its parent. It is how
you compose larger workflows out of reusable pieces — a child graph has its own nodes,
edges, channels and human gates, but appears to the parent as one node.

## Building one

`GraphBuilder.subgraph(id, child, options?)` nests a child built with its own
`createGraph(...)`:

```ts
import { createGraph } from "@ailu/graph-sdk";

const child = createGraph({ name: "double", id: "double" })
  .channel("in", { type: "number", default: 0 })
  .channel("out", { type: "number", default: 0 })
  .node("calc", async (_input, state) => ({ out: (state.channels.in ?? 0) * 2 }));

const parent = createGraph({ name: "parent" })
  .channel("x", { type: "number", default: 21 })
  .channel("y", { type: "number", default: 0 })
  .subgraph("sub", child, { inputMapping: { in: "x" }, outputMapping: { y: "out" } });

const result = await parent.compile().run({ x: 21 });
// result.channels.y === 42
```

## Channel mapping

- **`inputMapping`** — `{ childChannel: parentChannel }`. Each entry projects a parent
  channel into the child's initial state. Omit it and the child receives a copy of every
  parent channel.
- **`outputMapping`** — `{ parentChannel: childChannel }`. On completion each entry writes
  a child channel back into the parent. Omit it and every child channel is merged onto the
  parent (child wins on collisions).

Declare on the parent any channel an `outputMapping` writes into.

## Suspension propagates

If the child suspends — for example it hits an **internal human gate** — the parent
suspends at the subgraph node. The parent's next `resume` re-attaches to the *same* child
run and continues it (the child's suspended state round-trips in the parent state, so this
holds even across separate engine calls / process restarts), then maps the child's output
back out and routes on.

```ts
const suspended = await parent.compile().run({});
// suspended.status === "suspended"  (the child's gate)
const done = await app.resume(suspended.runId);
// the child advanced past its gate, the parent continued
```

## Notes

- Child runs **share the parent's** node/condition registries, checkpointer and event bus.
  Child node ids must therefore be unique across the parent (a collision is a compile error).
- Subgraphs run on the Rust engine and the in-process TypeScript engine identically.
