---
sidebar_position: 3
title: Troubleshooting
description: A typed-error index and the operational failure modes you will actually hit.
---

# Troubleshooting

Ailu errors are **typed classes**, never bare `throw new Error("…")`. The class name
and message tell you the cause; this page maps each to its fix, then covers the
engine-level footguns (engine availability, cross-process resume, validation codes, and
mock sequencing).

## Typed-error index

| Error | Where from | Cause → fix |
| --- | --- | --- |
| `GraphValidationError` | `@ailu/graph-core` (`errors.ts`) | A graph fails validation. Carries a `code` (`DUPLICATE_NODE_ID`, `DUPLICATE_EDGE_ID`, `MISSING_ENTRY_NODE`, `INVALID_EDGE_REFERENCE`, `CYCLE_DETECTED`, `INVALID_CONDITION_FORMAT`) and a `path`. → Fix the structural issue the code names. |
| `GraphCompileError` | `@ailu/graph-sdk` (`errors.ts`) | `.compile()` was called on a graph that failed validation. Wraps the underlying `GraphValidationError[]` in `.errors`. → Read each entry's `code: message`, or call `safeCompile()` for a `Result` instead of a throw. |
| `DuplicateNodeError` | `@ailu/graph-sdk` | Two nodes added under the same id. → Give each node a unique id. |
| `MissingHandlerError` | `@ailu/graph-sdk` | An action node was added with no executable handler. → Provide a handler (or use the right node helper — `humanGate`, `agentNode` — which supply their own). |
| `RecursionLimitError` | `@ailu/graph-runtime` (`cycles.ts`) | A cyclic graph exceeded its `recursionLimit`. → Raise the limit if the loop is legitimate, or fix the condition that never exits. |

:::note On `RunError`
There is no `RunError` class. A run that fails surfaces as a **`run_failed` event**
(`{ type: "run_failed", runId, error, timestamp }`) on the event bus, and the run's
status becomes `failed`. To diagnose, read the run's event journal (subscribe with
`app.onEvent(...)`, or replay the journal you persisted) and inspect the `error` string on
the `run_failed` event — it carries the message of whatever the failing node threw. The
`AiluSdkError` base class covers SDK-thrown errors; `ToolException` (graph-runtime) is
thrown by a failing tool node.
:::

All SDK errors extend `AiluSdkError`, so `error instanceof AiluSdkError` catches
the lot.

## `rustEngineAvailable()` is `false`

The Rust engine loads through the native addon `@ailu/napi`. The loader
(`graph-sdk/src/rust-engine.ts`) `require`s it and returns `null` if the require throws
or the module lacks the run/resume/approve bridge — which is exactly what happens on a
platform with no prebuilt binary (musl/Alpine, Windows arm64, or any host you have not
built for).

When the addon is absent, `rustEngineAvailable()` is `false` and `CompiledGraph` falls
back to the **in-process TypeScript engine** automatically — the public SDK API is
unchanged across engines. The TS engine is the dev/test/uncovered-platform path; it is
not deprecated and the API surface is identical.

```ts
import { rustEngineAvailable } from "@ailu/graph-sdk";
console.log(rustEngineAvailable());
```

Expected result: `true` once the addon is built (`bash scripts/build-napi.sh`), `false`
on an uncovered platform — in which case runs still work on the TS engine.

:::warning Catalog graphs require Rust
A plain `GraphDefinition` run via `runCatalogGraph()` (the control plane's seam — agents
and components run natively, no in-process handlers) **requires** the addon. If it is
absent, the seam throws `RustEngineUnavailableError`. Build the addon, or author the
graph with the SDK builder so it has TS handlers to fall back to.
:::

## `resume()` fails in a new process

A run resumes from its latest checkpoint. With `InMemoryCheckpointer` those checkpoints
live in the original process's memory and are gone the moment a *different* process
tries to resume.

Fix: give the graph a **durable** `Checkpointer` so checkpoints are cross-process. The
engine ships the `Checkpointer` interface + `InMemoryCheckpointer` only — implement the
interface against your own store (Postgres/Redis/…), or use **Ailu Studio**, the managed
control plane that provides durable checkpointing for you. Then resume on the **same
`CompiledGraph`** that produced the checkpoint: the checkpoint encodes a position in a
specific graph, so resuming against a structurally different graph (renamed nodes, changed
edges) will not line up. See
[persistent checkpointing](/docs/core-concepts/resumability-and-approvals).

## Mock-sequencing order for scripted gateways

`MockLLMProviderAdapter` replays its `responses` array **one per `complete()` call**, in
order, and **repeats the last entry** once the array is exhausted (`mock-adapter.ts`).
For a multi-turn agent (a `tool_use` turn, then a final-answer turn) the order in the
array is the order the turns are consumed — get it wrong and the agent reads the
final-answer turn where it expected a tool call.

```ts
new MockLLMProviderAdapter({
  provider: "anthropic",
  responses: [
    /* turn 1 */ { content: "…tool_use…", usage: { promptTokens: 0, completionTokens: 0 }, model: "mock", provider: "anthropic" },
    /* turn 2 */ { content: "FINAL: done.", usage: { promptTokens: 0, completionTokens: 0 }, model: "mock", provider: "anthropic" }
  ]
});
```

Expected result: the first `complete()` returns turn 1, the second returns turn 2, and
any further calls repeat turn 2. A single `response` (not `responses`) is equivalent to
a one-element array that repeats forever. `responses` takes precedence over `response`.

## No live SSE view from the engine

The engine emits events in-process; it does **not** serve an HTTP stream. If you expected a
live `EventSource`/SSE view, that belongs to a control plane, not the library. Subscribe with
`app.onEvent(...)` and push events to your own transport, or use **Ailu Studio**, which
persists the journal and serves the live SSE governance view for you. See
[observable runs](/docs/governance/observable-runs).

## See also

- [Running in production](/docs/production/deployment)
- [Production best practices](/docs/production/best-practices)
- [The execution contract](/docs/core-concepts/execution-contract)
