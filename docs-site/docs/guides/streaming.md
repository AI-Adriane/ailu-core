---
title: "Streaming"
description: "Stream a run's tokens, node updates or full state as it executes, and subscribe to run events."
---

# Streaming

`app.stream(input, mode)` runs the graph like `run()` and yields events as it goes. Pick the
mode for what you want to show.

Tokens as the model writes them:

```ts file=packages/graph-sdk/examples/docs/streaming.ts region=messages
```

Progress, node by node:

```ts file=packages/graph-sdk/examples/docs/streaming.ts region=updates
```

## Modes

| Mode | Yields | Use it for |
| --- | --- | --- |
| `"messages"` | `message_delta`: `{ delta, nodeId, messageId }` for each token of each agent | A chat UI |
| `"updates"` | `state_update`: `{ nodeId, delta }` when a node finishes | A progress list |
| `"values"` | `state_value`: `{ state }`, the full state after each node | Debugging, dashboards |
| `"debug"` | `debug`: every lifecycle event, wrapped | Tracing |

`messageId` groups the tokens of one agent turn, so you can show several agents side by side.

A stream only yields events of its own run, even when several runs of the same graph stream at
once. Leaving the loop early with `break` is safe.

## Subscribe to events

`app.onEvent(handler)` receives the lifecycle events of every run of the graph: `node_started`,
`node_completed`, `run_suspended`, `run_completed`, `run_failed`, and more. It returns an
unsubscribe function. The full list is in [Events](../reference/events.md); sending them to a
tracing backend is covered in [Observability](./observability.md).

## Next

- [Observability](./observability.md): traces, costs, the browser inspector.
