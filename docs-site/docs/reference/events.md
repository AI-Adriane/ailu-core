---
title: "Events"
description: "The run lifecycle events and the stream events, with their fields."
---

# Events

## Run events

Subscribe with `app.onEvent(handler)`, or `onEvent` in the catalog runner's options. Every event
has `type`, `runId` and `timestamp`.

| `type` | Extra fields | When |
| --- | --- | --- |
| `node_started` | `nodeId` | A node starts. |
| `node_completed` | `nodeId`, `output` | A node finishes; `output` is the update it returned. |
| `node_failed` | `nodeId`, `error`, `attempt`, `category` | A node threw. With a `retryPolicy`, once per attempt. |
| `node_error_routed` | `nodeId`, `toNodeId`, `errorEdgeId`, `error`, `category` | Retries are spent and the run follows an error edge. |
| `run_suspended` | `nodeId`, `reason` | The run stopped: `human-gate`, `interrupt` (tool approval), `timer` or `signal`. |
| `run_resumed` | `nodeId` | A suspended run continues. |
| `run_completed` | `finalState` | The run finished. |
| `run_failed` | `error` | The run failed. |
| `run_cancelled` | `nodeId` | The run was cancelled before `nodeId`. |
| `token_delta` | `nodeId`, `messageId`, `delta`, `parentRunId`, `spawnId` | A model token, when token streaming is on. `parentRunId` and `spawnId` are set only for a `mapAgents` sub-agent. Not saved in checkpoints. |

`category` classifies a failure, so an error branch can treat a transient error (a timeout) and a
permanent one differently.

Values of channels marked `noLog: true` are masked in every event.

## Stream events

`app.stream(input, mode)` yields one of these, depending on `mode`:

| Mode | `type` | Fields |
| --- | --- | --- |
| `"messages"` | `message_delta` | `delta` (text), `nodeId`, `messageId` |
| `"messages"` | `tool_call` | `toolId`, `input`, `nodeId`: a tool call the model made |
| `"updates"` | `state_update` | `nodeId`, `delta` (the channels the node changed) |
| `"values"` | `state_value` | `state` (the full state) |
| `"debug"` | `debug` | `nodeId`, `payload` (a run event) |

See [Streaming](../guides/streaming.md).
