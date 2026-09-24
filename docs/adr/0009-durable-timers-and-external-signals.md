# ADR 0009 — Durable timers & external signals as generalized suspend reasons

- Status: Proposed
- Date: 2026-06-20
- Builds on: [ADR 0008](0008-rust-runtime-parity-fanout-subgraphs-streaming.md) (Rust runtime parity), [ADR 0003](0003-ts-engine-deprecated-sdk-on-rust.md) (Rust is canonical)

## Context

Ailu had no Temporal-style durable timers or external signals: a graph could not "sleep for an hour then continue" or "wait until an external event arrives." The only ways a run paused were a human gate and a dynamic interrupt (agent approval). Long-running, event-driven workflows (wait-for-payment, wait-for-webhook, scheduled follow-ups) were impossible.

The insight: a durable timer and an external signal are just **two more reasons a run suspends**, on top of the human gate and the dynamic interrupt the engine already supports. The suspend → checkpoint → resume machinery is solid; generalizing it is low-risk and high-leverage.

A hard constraint: the engine must stay **deterministic and clock-free**. It already reads no wall clock for routing decisions (retry `backoffMs` is read but the crate never sleeps). Timers must preserve that — the *scheduling* of a wake-up belongs to the embedder (the control-plane worker), not the engine.

## Decision

Add two suspend reasons to the canonical Rust runtime, reusing the existing suspend/resume/checkpoint path.

### 1. Durable timer (`sleep_until`)

`NodeOutput.sleep_until: Option<String>` — when a node returns it, the runtime applies the node's update, emits `NodeCompleted`, records suspend metadata, and **suspends** with reason `"timer"`. `wake_at` is an **opaque deadline string** (ISO-8601 / epoch-millis — the engine never parses or compares it to a clock). On resume the run **advances past** the node (a one-shot, like a gate opening), unlike a dynamic interrupt which re-runs the node. The control-plane worker reads `wake_at` off the suspended run and calls `resume` at that time.

### 2. External signal (`wait_for_signal`)

`NodeOutput.wait_for_signal: Option<String>` — suspends with reason `"signal"`, recording the awaited signal name. A new seam `GraphRuntime::resume_with_signal(run_id, name, payload)` (the twin of `update_state` + `resume`) injects the payload into the `__signals[name]` channel and resumes, advancing past the waiting node. Downstream nodes read the payload from `__signals`.

### 3. Signal-or-timeout

A node may return both (`NodeOutput::wait_for_signal_or_timeout`): it suspends as a signal wait that also carries a `wake_at`. Whichever fires first wins — `resume_with_signal` (signal arrived) or a plain `resume` at the deadline (timeout). Downstream inspects whether `__signals[name]` was populated to tell which path ran. No special engine logic — both are ordinary resumes that advance.

### 4. Suspend metadata + generalized resume

A `__suspend` channel carries `{ reason, wakeAt?, awaitingSignal? }` on a timer/signal suspension — the scheduler's contract for *when* to resume a timer and *which* signal to deliver. `resume` advances past the suspended node when it is a human gate **or** the suspend reason is `"timer"`/`"signal"`; a dynamic interrupt still re-runs. The metadata is cleared when the suspension resolves.

### 5. napi + SDK surface

- napi: `engine_signal(specJson, signalName, payloadJson, …)` wraps `resume_with_signal`; timers need no new entry point (the worker calls `engine_resume` at `wakeAt`). The JS node seam recognises two reserved update keys — `__sleepUntil` and `__waitForSignal` — so a JS handler requests a timer / signal wait without a structured return.
- SDK (`@ailu/graph-sdk`): handler helpers `sleepUntil(wakeAt, update?)` and `waitForSignal(name, { wakeAt?, update? })`; `CompiledGraph.signal(runId, name, payload)`; and `readSuspendMeta(state)` / `readSignal(state, name)` for the consumer/scheduler. These run on the **Rust engine** — `signal()` throws a clear error on the TS dev fallback (which does not model timers/signals), consistent with ADR 0003's Rust-only carve-outs.

## Consequences

- **Public API (Rust).** `NodeOutput` gains `sleep_until` / `wait_for_signal` (+ `sleep` / `wait_for_signal` / `wait_for_signal_or_timeout` constructors); `GraphRuntime::resume_with_signal` is new. Reserved channels: `__suspend`, `__signals`.
- **Public API (napi/SDK).** New `engineSignal` addon fn (rebuild required); new SDK exports (`sleepUntil`, `waitForSignal`, `readSuspendMeta`, `readSignal`, `CompiledGraph.signal`).
- **Invariants preserved.** The engine never sleeps or reads a clock for decisions — `wakeAt` is data, the worker schedules the wake. Checkpoint after suspend, event per transition, clean resume — unchanged. Timers/signals are suspend reasons, not a new control-flow path.
- **Handoff to the control plane (product, not in this change).** A scheduler that resumes timer runs at `wakeAt` (e.g. BullMQ delayed jobs), and an API endpoint `POST /runs/:id/signals/:name` that calls `engineSignal`. The engine exposes everything they need (`__suspend.wakeAt`, `engine_signal`); wiring them is product work.
- **Deferred (tracked):** delivering a signal awaited *inside a subgraph* (the payload lands in the top-level run's `__signals`); porting timers/signals to the deprecated TS engine (Rust is canonical — the TS fallback throws / does not suspend on the markers).
