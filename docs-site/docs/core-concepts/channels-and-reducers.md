---
sidebar_position: 2
title: Channels and reducers
description: How state is declared and how node outputs are merged into it.
---

# Channels and reducers

State in Ailu is not a single blob — it's a set of named **channels**, each with its own
type, default, and **reducer**. The reducer is the rule for merging a node's output into the
channel. Choosing the right reducer is most of what "modelling state" means in Ailu.

## Declaring a channel

```ts
createGraph({ name: "demo" })
  .channel("status", { type: "string", default: "new" })
  .channel("messages", { type: "messages", reducer: "append", default: [] })
  .channel("totals", { type: "object", reducer: "merge", default: {} });
```

- **`type`** — the channel's value type. The graph's state type is inferred from your channels,
  so `result.channels` is fully typed.
- **`default`** — the value before any node writes to it.
- **`reducer`** — how an update is applied (below). Defaults to `"replace"`.

## The three reducers

| Reducer | Merge rule | Use it for |
| --- | --- | --- |
| **`replace`** (default) | The update overwrites the current value. | scalars, status flags, the latest result |
| **`append`** | The update is appended to the existing list. | conversational `messages`, event logs, accumulating items |
| **`merge`** | Object update is shallow-merged into the current object. | partial updates to a record (totals, flags) |

A node returns a **partial** channel map; only the channels it names are touched, each through
its own reducer:

```ts
.node("step", async () => ({
  status: "done",                 // replace → status becomes "done"
  messages: [{ role: "assistant", content: "ok" }], // append → added to the list
  totals: { charged: 1 }          // merge → merged into the totals object
}))
```

## Messages channels

Conversational state is common enough to have a shorthand:

```ts
createGraph({ name: "chat" })
  .messagesChannel()   // == .channel("messages", { type: "messages", reducer: "append", default: [] })
```

`.messagesChannel(name?)` declares an append-reduced messages channel. Agent nodes read and
append to it as they reason.

## Why reducers (and not just assignment)

Reducers make merges **deterministic and replayable**. Because the rule for combining state is
declared up front — not decided ad hoc inside a handler — the runtime can checkpoint a channel
and re-apply updates on resume without ambiguity. Append-vs-replace is a property of the
channel, not of whoever happened to write to it.

## Next

- [The execution contract](./execution-contract)
- [Resumability and approvals](./resumability-and-approvals)
