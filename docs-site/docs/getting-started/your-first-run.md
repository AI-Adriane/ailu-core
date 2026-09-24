---
sidebar_position: 2
title: Your first run
description: Build a graph from scratch — channels, action nodes, edges, conditional routing — and run it.
---

# Your first run

Build a graph from scratch: declare typed **channels**, add **action nodes**, wire them with
**edges** (including a conditional edge with a named predicate), and run it. By the end you'll
understand the builder, channels and reducers, and how routing works.

Prerequisites: [Installation](./installation).

## The builder in one breath

`createGraph(options)` returns a `GraphBuilder`. You chain `.channel(...)`, `.node(...)`,
`.edge(...)` / `.conditionalEdge(...)`, then `.compile()` to get a runnable `CompiledGraph`.
The first node you add becomes the entry point unless you call `.entry(id)`.

## A linear graph

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "order-flow" })
  .channel("amount", { type: "number", default: 0 })
  .channel("status", { type: "string", default: "new" })
  .node("validate", async (_input, state) => {
    if (state.channels.amount <= 0) {
      throw new Error("amount must be positive");
    }
    return { status: "validated" };
  })
  .node("charge", async () => ({ status: "charged" }))
  .edge("validate", "charge")
  .compile();

const result = await app.run({ amount: 42 });
console.log(result.status);          // "completed"
console.log(result.channels.status); // "charged"
```

**Expected result:** the run flows `validate → charge`, finishes `completed`, and `status` is
`"charged"`.

### What each call does

- **`createGraph({ name })`** — `name` is required; `id` defaults to a slugified name and
  `version` defaults to `"0.0.0"`. You can also pass `recursionLimit` and `metadata`.
- **`.channel(name, { type, reducer?, default? })`** — declares a typed state channel. The
  value type is inferred from `default` and accumulates into the graph's state type, so the
  result of `run()` is fully typed. `reducer` defaults to `"replace"`.
- **`.node(id, handler)`** — adds an **action** node. The handler is
  `async (input, state, context) => update`, where `update` is a partial channel map
  (`{ status: "charged" }`). Returning `{}` is a valid no-op.
- **`.edge(from, to)`** — an unconditional edge that is always followed.
- **`.compile()`** — validates and returns the runnable graph. It throws a `GraphCompileError`
  if validation fails; use `.safeCompile()` to get a `Result` instead of throwing.

## Channels and reducers

A channel's `reducer` decides how a handler's update is merged into state:

- **`"replace"`** (default) — the update overwrites the current value.
- **`"append"`** — the update is appended to a list (used for conversational `messages`).
- **`"merge"`** — object updates are merged.

For a conversational `messages` channel there's a shorthand:

```ts
const app = createGraph({ name: "chat" })
  .messagesChannel()           // declares an append-reduced "messages" channel (default [])
  .node("greet", async () => ({ messages: [{ role: "assistant", content: "Hi!" }] }))
  .compile();
```

`.messagesChannel(name?)` is equivalent to
`.channel(name, { type: "messages", reducer: "append", default: [] })`. More on this in
[Channels and reducers](/docs/core-concepts/channels-and-reducers).

## Conditional routing with a named predicate

Routing decisions are **named predicates you register** — never `eval`'d strings. This is what
keeps flows safe and inspectable. You register a predicate with `.conditionalEdge(...)`:

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "triage" })
  .channel("amount", { type: "number", default: 0 })
  .channel("route", { type: "string", default: "" })
  .node("intake", async (_input, state) => ({
    route: state.channels.amount > 1000 ? "manual" : "auto"
  }))
  .node("auto-approve", async () => ({ route: "auto-done" }))
  .node("manual-review", async () => ({ route: "manual-done" }))
  // Two guarded edges out of `intake`: the predicate that holds wins.
  .conditionalEdge("intake", "manual-review", "isLargeAmount", (s) => s.channels.amount > 1000)
  .conditionalEdge("intake", "auto-approve", "isSmallAmount", (s) => s.channels.amount <= 1000)
  .compile();

const big = await app.run({ amount: 5000 });
console.log(big.channels.route);   // "manual-done"

const small = await app.run({ amount: 50 });
console.log(small.channels.route); // "auto-done"
```

**Expected result:** `amount > 1000` routes to `manual-review`; otherwise to `auto-approve`.

`.conditionalEdge(from, to, conditionName, predicate)` registers `predicate` under
`conditionName` and adds a conditional edge guarded by it. The predicate receives the live,
typed state and returns a boolean. Why named-not-`eval`: see the
[execution contract](/docs/core-concepts/execution-contract).

## Setting the entry point explicitly

The first node added is the entry node. Override it when you add nodes in a different order:

```ts
createGraph({ name: "g" })
  .node("b", async () => ({}))
  .node("a", async () => ({}))
  .entry("a")   // run starts at "a", not "b"
  .edge("a", "b")
  .compile();
```

## Inspecting the result

`run()` resolves to a typed `TypedGraphState`. The fields you'll use most:

- `status` — `"completed"`, `"suspended"`, `"failed"`, etc.
- `channels` — the typed channel map (your declared channels).
- `runId` — the run id (auto-generated unless you pass `{ runId }` to `run`).
- `currentNodeId` — where execution paused (relevant when `status === "suspended"`).

## Try it

Adapt the shipped quickstart, which is the canonical minimal graph:

```bash
pnpm --filter @ailu-ai/graph-sdk example   # examples/quickstart.ts
```

## Next

- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) — add an LLM-driven agent.
- [Core concepts](/docs/core-concepts/graphs-nodes-edges-state) — the model in depth.
