---
sidebar_position: 3
title: Events and streams
description: The RunEvent lifecycle union, the StreamEvent union and its four modes, and the Rust single-terminal-event caveat.
---

# Events and streams

Ailu exposes two distinct observation surfaces:

- **`RunEvent`** — the run lifecycle journal, one event per node/run transition, subscribed via
  [`CompiledGraph.onEvent`](/docs/reference/builder-api#oneventhandler). This is the **audit
  trail** the [execution contract](/docs/core-concepts/execution-contract) guarantees.
- **`StreamEvent`** — incremental run output, consumed by iterating
  [`CompiledGraph.stream`](/docs/reference/builder-api#streaminitialdata-mode-options).

They are different unions for different purposes. `RunEvent` is defined in
`packages/graph-runtime/src/types.ts`; `StreamEvent` and `StreamMode` in
`packages/graph-runtime/src/stream.ts`.

## RunEvent (the lifecycle journal)

Every transition emits exactly one event. If a transition happened, there is an event for it; if
there is no event, it did not happen.

```mermaid
sequenceDiagram
  participant R as Run
  participant Bus as Event bus
  R->>Bus: node_started (per node)
  R->>Bus: node_completed (or node_failed)
  Note over R,Bus: ...repeats per node...
  alt human gate / approval
    R->>Bus: run_suspended
    R->>Bus: run_resumed
  end
  R->>Bus: run_completed (or run_failed)
```

The `RunEvent` union:

| `type` | Payload fields | Fires when |
| --- | --- | --- |
| `node_started` | `runId`, `nodeId`, `timestamp` | A node begins executing. |
| `node_completed` | `runId`, `nodeId`, `output`, `timestamp` | A node finishes successfully (`output` is its result). |
| `node_failed` | `runId`, `nodeId`, `error`, `attempt`, `timestamp` | A node throws (`error` is the message, `attempt` the retry count). |
| `run_suspended` | `runId`, `nodeId`, `reason`, `timestamp` | The run pauses cleanly — at a human gate, or an agent suspending for approval (`reason` carries why; `nodeId` is where it paused). |
| `run_resumed` | `runId`, `nodeId`, `timestamp` | A suspended run resumes from its latest checkpoint. |
| `run_completed` | `runId`, `finalState`, `timestamp` | The run reaches a terminal state (`finalState` is the full `GraphState`). |
| `run_failed` | `runId`, `error`, `timestamp` | The run fails irrecoverably (`error` is the message). |

All `timestamp` values are ISO strings; `runId` / `nodeId` are branded types from
`@ailu/graph-core`.

```ts
const off = app.onEvent((event) => {
  if (event.type === "node_completed") {
    console.log(`${event.nodeId} done`, event.output);
  }
});
await app.run({});
off(); // unsubscribe
```

Expected result: prints one line per completed node, then unsubscribes.

:::note Events arrive identically across engines
On the Rust path, forwarded engine events are mirrored into the same event bus the TS path uses,
so `onEvent` subscribers see the same `RunEvent` stream regardless of engine. (Source:
`compiled-graph.ts` — the runner's `subscribe` re-emits into the shared bus.)
:::

## StreamEvent (incremental output)

`stream(initialData, mode, options?)` returns an `AsyncIterable<StreamEvent>`. The union:

| `type` | Payload fields | Meaning |
| --- | --- | --- |
| `state_value` | `state` | A full `GraphState` snapshot. |
| `state_update` | `delta`, `nodeId` | The channel-update map a node produced. |
| `message_delta` | `delta`, `nodeId`, `messageId` | A streamed token chunk for a message. |
| `tool_call` | `toolId`, `input`, `nodeId` | A tool invocation a node emitted. |
| `debug` | `payload`, `nodeId` | Arbitrary debug payload. |

### The four stream modes

`StreamMode` is one of `"values" | "updates" | "debug" | "messages"`
(`STREAM_MODES` in `stream.ts`):

| Mode | Emits | Use for |
| --- | --- | --- |
| `values` | `state_value` snapshots | Watching full state evolve. |
| `updates` | `state_update` deltas | Reacting to per-node channel writes. |
| `messages` | `message_delta` token chunks | Live chat-style token output. |
| `debug` | `debug` (and detailed) events | Tracing/diagnostics. |

```ts
for await (const event of app.stream({ name: "Ada" }, "updates")) {
  if (event.type === "state_update") {
    console.log(event.nodeId, event.delta);
  }
}
```

Expected result (TS engine): prints one `state_update` per node as it writes. On the Rust
engine, see the caveat below.

## The Rust single-terminal-event caveat

:::warning Streaming is incremental only on the TS engine
The Rust engine has **no incremental stream surface yet**. When a graph runs on Rust,
`CompiledGraph.stream` drives a full run to its terminal state and yields **exactly one**
`state_value` event — the final state — and nothing else. Only the in-process TypeScript engine
streams incrementally (`updates`, `messages`, `debug` deltas). (Source: `compiled-graph.ts`,
`streamViaRust`.)
:::

So if you need live `message_delta` / `state_update` streaming during development, force the TS
engine with `AILU_SDK_ENGINE=ts`, or branch on
[`usesRustEngine`](/docs/reference/builder-api#usesrustengine). For per-transition observability
that **does** work on both engines, use the `RunEvent` journal via `onEvent` instead.

For token-by-token agent output specifically, the SDK also ships `streamAgentTokens(...)`, a
single-turn (no-tools) helper that streams text deltas straight through the LLM gateway —
independent of the graph stream surface.

## Next

- [Builder API](/docs/reference/builder-api)
- [Errors](/docs/reference/errors)
- [Observable runs](/docs/governance/observable-runs)
