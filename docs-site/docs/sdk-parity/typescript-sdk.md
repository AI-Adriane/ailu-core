---
sidebar_position: 2
title: TypeScript SDK
description: The full builder surface — graphs, agents, tools, streaming, custom handlers.
---

# TypeScript SDK

`@ailu/graph-sdk` is the front door to Ailu in TypeScript: the full builder, custom
node handlers, agents, tools, streaming, and the governance seams.

```bash
npm i @ailu/graph-sdk
```

```ts
import { createGraph } from "@ailu/graph-sdk";
```

It is a **self-contained bundle** and depends on the Rust engine (`@ailu/napi`), which is
installed automatically — Ailu runs on Rust. See
[runtime and engine](/docs/core-concepts/runtime-and-engine).

## The builder

`createGraph(options)` returns a `GraphBuilder`. Chain declarations and `.compile()` to a
runnable `CompiledGraph`.

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "order-flow", recursionLimit: 50 })
  .channel("amount", { type: "number", default: 0 })
  .channel("status", { type: "string", default: "new" })
  .node("validate", async (_input, state) => {
    if (state.channels.amount <= 0) throw new Error("amount must be positive");
    return { status: "validated" };
  })
  .node("charge", async () => ({ status: "charged" }))
  .edge("validate", "charge")
  .compile();

const result = await app.run({ amount: 42 });
```

The builder surface, in brief:

| Method | Adds |
| --- | --- |
| `.channel(name, opts)` / `.messagesChannel()` | a typed state channel |
| `.node(id, handler)` | a custom action node |
| `.agentNode(id, config)` | a ReAct agent node |
| `.toolNode(...)` | a node that executes tool calls |
| `.humanGate(id)` | a suspend-for-approval gate |
| `.edge(from, to)` | an unconditional edge |
| `.conditionalEdge(from, to, name, predicate)` | a guarded edge (named predicate) |
| `.entry(id)` | override the entry node |
| `.checkpointer(cp)` | set the checkpointer (the `Checkpointer` interface; ships `InMemoryCheckpointer`) |
| `.compile()` / `.safeCompile()` | validate → `CompiledGraph` (throws / returns a `Result`) |

## Running, suspending, resuming

```ts
const out = await app.run({ amount: 42 });        // TypedGraphState

// Suspends at a humanGate or for tool approval:
if (out.status === "suspended") {
  const done = await app.resume(out.runId);
  // or, for agent tool approval:
  // await app.approveAndResume(out.runId, { approvedTools: ["refund"] });
}
```

`run()` resolves to a typed `TypedGraphState` (`status`, `channels`, `runId`, `currentNodeId`).

:::note Durable resume
The open SDK ships the `Checkpointer` interface and an `InMemoryCheckpointer` (process-local).
For durable cross-process resume, implement the interface against your own store
(Postgres/Redis/…), or use **Ailu Studio** — the managed control plane that adds durable
checkpointing, a worker fleet, and the governance UI. The engine itself ships no Postgres
checkpointer.
:::

## Streaming and events

```ts
app.onEvent((event) => console.log(event.type, event.nodeId ?? ""));

for await (const chunk of app.stream({}, { mode: "updates" })) {
  // "values" | "updates" | "messages" | "debug"
}
```

See [streaming and events](/docs/building/streaming-and-events).

## Custom node handlers

A custom node is just an async function over the typed state, returning a partial channel map.
In TypeScript the engine bridges back into your JavaScript on every node through N-API. The C ABI
now exposes the same runtime shape through `AiluCallbacks`; Python remains JSON-in / JSON-out
until its PyO3 layer grows callbacks.

```ts
.node("score", async (_input, state, ctx) => {
  const risk = computeRisk(state.channels);
  return { risk };
})
```

`ctx` carries run context (the run id, emit helpers, the checkpointer). Operate **inside** the
[execution contract](/docs/core-concepts/execution-contract): do the work, return the update,
let the runtime checkpoint and emit.

## Errors

Typed error classes (never bare `throw`): `AiluSdkError`, `GraphCompileError`,
`DuplicateNodeError`, `MissingHandlerError`. Use `.safeCompile()` for a `Result` discriminated
union instead of a throw.

## Next

- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react)
- [One engine, many languages](./one-engine-two-languages)
