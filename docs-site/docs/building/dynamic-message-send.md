---
sidebar_position: 12
title: Dynamic messages (send / inbox)
description: Pre-queue per-node inputs and consume them one-per-execution — the map-reduce seam.
---

# Dynamic messages (`send` / inbox)

`send` lets you **pre-queue inputs for a node**. Each time that node executes it consumes
the next queued input (FIFO), exposed under the reserved `__injected` channel. Combined
with a cycle, this is the map-reduce / dynamic-dispatch primitive: queue N items for a
worker node and let it process them one pass at a time.

## Queuing inputs

Pass an `inbox` (per node id, a list of inputs) to `run`:

```ts
import { createGraph, readInjected } from "@ailu/graph-sdk";

const app = createGraph({ name: "map-reduce" })
  .channel("log", { type: "array", reducer: "append", default: [] as unknown[] })
  .channel("n", { type: "number", default: 0 })
  .node("worker", async (_input, state) => ({
    log: [readInjected(state)],
    n: ((state.channels.n as number) ?? 0) + 1
  }))
  // loop back to the worker until both items are processed
  .conditionalEdge("worker", "worker", "more", (s) => ((s.channels.n as number) ?? 0) < 2)
  .compile();

const result = await app.run({}, { inbox: { worker: ["first", "second"] } });
// result.channels.log === ["first", "second"]   (drained FIFO)
```

## Reading the injected input

A node handler reads the input it consumed this execution with `readInjected(state)`. The
value is visible to the handler only — it is **never persisted** into the run's channels.
When the node's inbox is empty, `readInjected` returns `undefined`.

## Notes

- The inbox is keyed per `(run, node)` and drained FIFO, one input per node execution.
- `RunOptions.inbox` runs on the Rust engine; on the in-process TypeScript fallback the
  injected value arrives as the handler's first (`input`) argument instead.
