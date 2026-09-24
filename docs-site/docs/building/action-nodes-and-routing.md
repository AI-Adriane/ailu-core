---
sidebar_position: 1
title: Action nodes & routing
description: Custom action nodes and conditional routing with named predicates.
---

# Action nodes & routing

An **action node** is the workhorse: an async function over the typed state that returns a
partial channel update.

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "order" })
  .channel("amount", { type: "number", default: 0 })
  .channel("status", { type: "string", default: "new" })
  .node("validate", async (_input, state) => {
    if (state.channels.amount <= 0) throw new Error("amount must be positive");
    return { status: "validated" };
  })
  .node("charge", async () => ({ status: "charged" }))
  .edge("validate", "charge")
  .compile();
```

The handler signature is `async (input, state, context) => update`. Returning `{}` is a valid
no-op. A handler can instead return a `Command { goto, update? }` to override default edge
resolution and jump explicitly.

## Conditional routing

Routing decisions are **named predicates you register** — never `eval`'d strings. Register one
with `.conditionalEdge(from, to, name, predicate)`:

```ts
createGraph({ name: "triage" })
  .channel("amount", { type: "number", default: 0 })
  .node("intake", async () => ({}))
  .node("auto-approve", async () => ({}))
  .node("manual-review", async () => ({}))
  .conditionalEdge("intake", "manual-review", "isLarge", (s) => s.channels.amount > 1000)
  .conditionalEdge("intake", "auto-approve", "isSmall", (s) => s.channels.amount <= 1000)
  .compile();
```

The predicate receives the live, typed state and returns a boolean. Multiple guarded edges out
of a node are evaluated in order; the first whose predicate holds is followed. Why named, not
`eval`: see [the execution contract](/docs/core-concepts/execution-contract).

## Next

- [Agent nodes & ReAct](./agent-nodes-and-react)
- [Channels and reducers](/docs/core-concepts/channels-and-reducers)
