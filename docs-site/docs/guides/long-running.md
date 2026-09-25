---
title: "Long-running runs"
description: "Resume a run in another process, wait for a date or an external event, and cancel a run cleanly."
---

# Long-running runs

A run can stop for a person, a date or an external event, and continue later. This page shows
where the state lives while it waits, and how to pick it up again.

## Two ways to run a graph

| | `app.run()` | `runCatalogGraph(app.definition)` |
| --- | --- | --- |
| State between steps | In the `CompiledGraph`, in memory | Returned to you as plain JSON |
| Resume | `app.resume(runId)`, same instance, same process | `resumeCatalogGraph(definition, state)`, anywhere |
| Nodes that run | All | Agents, components, human gates, subgraphs, `mapAgents`. Your own `.node()` functions and conditional-edge functions do not run. |
| Also | Streaming, timers and signals | Cancellation, record and replay |

Use `app.run()` while a run finishes within one process. Use the catalog runner when a run must
survive a restart or move between machines.

## Resume in another process

`runCatalogGraph` returns the state when the run stops. Store it anywhere that holds JSON, and
hand it to `resumeCatalogGraph` later:

```ts file=packages/graph-sdk/examples/docs/long-running-persist.ts region=example
```

To resume a tool approval this way, pass the approved tools in the resume options:
`resumeCatalogGraph(definition, state, { approvedTools: [{ name: "refund", requestedBy: "assistant", resolvedBy: "alice@example.com" }], tools, approvalEngine })`.
Pass your tool handlers again (`tools`) on every call: they are code, not state. With
`approvalEngine`, the resume first checks that the engine approved what the run waits on (see
[Governance](./governance.md#sign-approval-decisions)).

## Wait for an external event

A node can suspend the run until your system delivers a named signal: a payment webhook, a
message from another service, a manual action.

```ts file=packages/graph-sdk/examples/docs/durable-signal.ts region=example
```

`waitForSignal(name, { wakeAt })` also wakes the run at `wakeAt` if the signal never comes.

## Wait until a date

`sleepUntil(date)` suspends the run and records when to wake it. Ailu does not keep a clock
running: your scheduler (a cron job, a delayed queue message) calls `resume` at that time.

```ts file=packages/graph-sdk/examples/docs/durable-timer.ts region=example
```

`readSuspendMeta(state)` tells you why a run is suspended: `human-gate`, `interrupt` (a tool
approval), `timer` or `signal`, with `wakeAt` or `awaitingSignal`.

## Cancel a run

Pass an `AbortSignal` to the catalog runner. Aborting it stops the run at the next step: the
node in flight finishes, its checkpoint is saved, and the run ends with status `"cancelled"`.
A cancelled run can still be resumed or replayed.

```ts file=packages/graph-sdk/examples/docs/durable-cancel.ts region=example
```

## Next

- A complete example: [Resume across processes](../examples/resume-across-processes.md).
- Record a run to replay it later: [Governance](./governance.md#replay-a-run).
