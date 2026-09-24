# ADR 0044 — Cooperative run cancellation at a node boundary

- Status: **Proposed** — implemented in this PR, not yet released.
- Date: 2026-09-16
- Deciders: Mathieu (owner)
- Driven by the product repo's kill-switch work (`AI-Adriane/ailu` issue #1058, epic #1057 layer
  L2 / mechanism M6). That issue opened with the assumption that **no engine change was needed**;
  reading the code disproved it, and this ADR records why and what was added instead.

## Context

The product needs to stop a run that is *already executing* — a real kill switch, the blind spot
behind the OpenAI–Hugging Face incident (an agent swarm that ran for 13 hours and was detected by
the damage it did). The control plane could not do it, and the reason is structural:

- `runCatalogGraph` is **one atomic call** into the Rust engine: `runner.run(...)` executes the
  entire graph and only then returns. Nothing hands control back to the embedder between nodes.
- `RunCatalogGraphOptions` carried no cancellation of any kind — `{ runId, initialData, onEvent,
  streamTokens, approvalEngine, providerKeys, fsPolicy, skills, tools, subgraphs }`. The only
  `AbortSignal` anywhere in the SDK belongs to `HttpFetchRequestInit`, an unrelated HTTP type.
- `on_event` is fire-and-forget by construction (`bindings`: "`on_event` stays fire-and-forget
  (no return awaited)"). A sink that returns `void` **cannot** stop a run, so the existing event
  stream was not a usable seam.
- The only product-side alternative was killing the worker process. Workers run at
  `WORKER_CONCURRENCY=5`, so that would destroy four unrelated (possibly cross-tenant) runs to
  stop one. Unacceptable.

The key insight is that the engine's existing guarantee — **checkpoint after every node** — makes
cancellation *safe*, but safety is not the same as *reachability*. Stopping between two nodes
loses nothing, because the last checkpoint is authoritative; there was simply no way to ask.

## Decision

Add a **cooperative** cancellation seam, polled by the run loop at every node **boundary**.

1. **`GraphStatus::Cancelled` / `"cancelled"`** — a new terminal status in `graph-core` (Rust and
   TS) and in `@ailu/contracts`. Distinct from `failed` (nothing malfunctioned) and from the
   product's `rejected` (no gate was refused). Unlike `rejected` — which the control plane paints
   on top of a state the engine still calls `suspended` — this one is a genuine **engine** status:
   the persisted `GraphState` really is cancelled.
2. **`RunEvent::RunCancelled { runId, nodeId, timestamp }`** — one event per lifecycle transition,
   as the runtime contract requires. `nodeId` is the node the run was *about to* execute; it never
   ran. Emitted **after** the final checkpoint is durable, and never alongside
   `run_completed`/`run_failed`.
3. **`GraphRuntime::with_cancel_check(CancelCheck)`** — an injected `Fn() -> bool`, mirroring the
   existing `with_clock` / `with_subgraphs` builders. `run_loop` polls it at the top of each
   iteration: the previous node's checkpoint is already durable and the next node has not been
   scheduled, so it is the one instant where stopping costs nothing.
4. **`HostCallbacks::is_cancelled`** — a bridge callback shaped like `on_condition` (awaited,
   value-returning), because only a seam that can *answer* can stop a run. It is **defaulted to
   `false`**, so every existing embedder (the C API, tests, any out-of-tree host) compiles and
   behaves identically without opting in.
5. **`engine_run` / `engine_resume` / `engine_approve_and_resume` / `engine_signal` gain a
   trailing OPTIONAL `isCancelled` callback.** Optional (`Option<StringCallback>`) so a caller
   built against the previous four-argument shape keeps working unchanged. `engine_replay`
   deliberately does **not** take it: a replay is a deterministic re-derivation of a run that
   already happened, and a live cancel flag must never change what it reproduces, or
   verify-replay would stop being evidence.
6. **`RunCatalogGraphOptions.signal?: AbortSignal`** — the public front door. Abort it and the run
   stops at the next node boundary, returning `CatalogRunOutcome { status: "cancelled", state }`.
   An already-aborted signal stops the run before its first node executes.
7. **Subgraphs propagate.** A child run shares the parent's runtime and therefore the same seam.
   `execute_subgraph` and the `map_subgraph` fan-out both propagate a cancelled child upward
   instead of treating it as completed — a parent must never advance past a subgraph node whose
   child did not finish. Cancellation also wins over suspension in a fan-out: a cancelled run must
   not park on a gate that no longer has any reason to be decided.

## Consequences

- **Cancellation is not instantaneous, by design.** Its latency is the duration of the node in
  flight; a node already executing (an agent mid-LLM-call) always runs to completion. This is the
  price of never tearing state, and it is the right trade: the alternative breaks the
  checkpoint-per-node guarantee the whole system rests on.
- **The engine offers no hard abort.** A caller needing a *bounded* stop must impose its own
  deadline around the call. This is stated in the `signal` doc comment so no embedder assumes a
  guarantee that does not exist.
- **Every runtime invariant is preserved**: determinism (the seam is polled, never racing mid-node),
  checkpoint-after-every-node (the cancel path checkpoints before it emits), an event per lifecycle
  transition (`run_cancelled`), and resumability — a cancelled run resumes or replays from its last
  checkpoint like any other.
- **Nothing changes for callers who do not opt in.** No signal, no `is_cancelled` implementation,
  or an older addon: the loop is byte-for-byte the previous behaviour.
- **The TS `GraphRuntime` (legacy in-process fallback) does not implement the seam.** It never
  produces `cancelled`; the status is simply unreachable there. Honest and additive — the Rust
  engine is the production runtime. Extending it is a follow-up if the fallback ever needs it.
