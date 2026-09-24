---
sidebar_position: 11
title: Durable timers and signals
description: Pause a run until a deadline or an external event — without the engine ever sleeping.
---

# Durable timers and signals

A node can pause its run two more ways beyond a human gate or a dynamic interrupt:

- **a durable timer** — wait until a deadline, then continue;
- **an external signal** — wait until something outside delivers a named event.

Both are just *suspend reasons*: the run checkpoints and stops, exactly like a human gate.
The engine itself **never sleeps and reads no clock** — `wakeAt` is opaque data, and a
scheduler (the control-plane worker) resumes the run when the time comes. This keeps runs
deterministic and resumable across process restarts.

> These run on the **Rust engine** (the production runtime). The in-process TypeScript
> fallback does not model them — `CompiledGraph.signal` throws there.

## Durable timer

Return `sleepUntil(wakeAt, update?)` from a node handler. The node's `update` is applied,
then the run suspends; on resume it advances past the node (one-shot).

```ts
import { sleepUntil, readSuspendMeta } from "@ailu/graph-sdk";

app = createGraph({ name: "follow-up" })
  .channel("step", { type: "string", default: "" })
  .node("wait", async () => sleepUntil("2026-01-01T00:00:00Z", { step: "sleeping" }))
  .node("send", async () => ({ step: "sent" }))
  .edge("wait", "send")
  .compile();

const suspended = await app.run({});
readSuspendMeta(suspended); // { reason: "timer", wakeAt: "2026-01-01T00:00:00Z" }
// the scheduler resumes at wakeAt:
const done = await app.resume(suspended.runId);
```

`wakeAt` is whatever string your scheduler understands (ISO-8601, epoch-millis). The engine
stores it; your worker reads it from `readSuspendMeta(state).wakeAt` and calls `resume` then.

## External signal

Return `waitForSignal(name, options?)` to wait for a named event. Deliver it with
`CompiledGraph.signal(runId, name, payload)`; the payload lands in the `__signals` channel,
readable with `readInjected`-style `readSignal`.

```ts
import { waitForSignal, readSignal } from "@ailu/graph-sdk";

app = createGraph({ name: "await-approval" })
  .channel("received", { type: "string", default: "" })
  .node("wait", async () => waitForSignal("approval"))
  .node("after", async (_i, state) => ({ received: String(readSignal(state, "approval") ?? "") }))
  .edge("wait", "after")
  .compile();

const suspended = await app.run({});          // status: suspended, awaitingSignal: "approval"
const done = await app.signal(suspended.runId, "approval", "yes");
// done.channels.received === "yes"
```

## Signal *or* timeout

Pass a `wakeAt` to `waitForSignal` for whichever-fires-first. The run wakes on the signal
*or* at the deadline; downstream inspects whether the signal channel was populated to tell
which path ran.

```ts
waitForSignal("approval", { wakeAt: "2026-01-01T00:00:00Z" });
```

## The scheduler contract

A suspended run carries everything the control plane needs in `readSuspendMeta(state)`:

| Field | Meaning |
| --- | --- |
| `reason` | `"timer"` or `"signal"` |
| `wakeAt` | when to `resume` a timer (or the signal-or-timeout deadline) |
| `awaitingSignal` | the signal name a `signal(...)` must deliver |

The worker resumes timer runs at `wakeAt`; a `POST /runs/:id/signals/:name` endpoint maps
to `signal(...)`.
