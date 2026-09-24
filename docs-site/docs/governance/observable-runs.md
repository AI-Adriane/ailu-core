---
sidebar_position: 4
title: Observable runs
description: The event journal as audit trail and as the basis for live governed-run views.
---

# Observable runs

The [execution contract](/docs/core-concepts/execution-contract) promises an **event for every
node lifecycle transition**. That event stream is not telemetry you opt into — it is the
backbone of both the audit trail and any live view of a run.

## The event vocabulary

Every run produces a sequence of typed events. The ones you'll watch for:

| Event | Emitted when |
| --- | --- |
| `node_started` / `node_completed` | A node begins / finishes. |
| `run_suspended` | The run parks at a human gate or for tool approval. |
| `run_resumed` | A resume picks the run back up. |
| `run_completed` / `run_failed` | The run ends. |

Because the journal is ordered and complete, **replaying it reconstructs the run exactly** —
which is what makes it usable both for audit and for a live UI.

## Subscribing to events

```ts
const app = createGraph({ name: "publish-flow" })
  // …nodes…
  .compile();

app.onEvent((event) => {
  console.log(event.type, event.nodeId ?? "");
});

await app.run({});
// node_started write … node_completed write … run_suspended review …
```

## The journal is the audit trail

There is no separate, hand-maintained audit log to drift out of sync. The same events that
drive a live view *are* the record of what happened: when the run suspended, which approval was
pending, who resolved it, and when it resumed. Paired with
[attestation](./tool-approval-and-attestation#attestation), each governance decision in that
journal is tamper-evident.

## Live governed-run views

The engine **emits** these events; persisting them and serving a live view is the job of a
control plane on top — **Ailu Studio** (the managed governance platform), or one you build on
the SDK. A control plane persists the journal and **replay-then-tails** it over a stream (e.g.
Server-Sent Events): a new observer first receives the historical events to catch up, then
follows new ones live. This is exactly how a governance dashboard shows a run suspending at a
gate, surfaces the pending approval, and then shows it resuming the instant a human approves —
all driven by the same events the engine already emits.

:::note
A faithful live view replays the persisted journal rather than re-executing the graph — so what
an observer sees is precisely what happened, never a re-run.
:::

## Next

- [The execution contract](/docs/core-concepts/execution-contract)
- [The governance model](./governance-model)
