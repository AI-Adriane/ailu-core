---
title: "Observability"
description: "Watch runs as they happen, send traces and costs to your tracing backend, and debug a stuck or failed run."
---

# Observability

## Run events

Every run emits lifecycle events. Subscribe with `onEvent`:

```ts file=packages/graph-sdk/examples/docs/observability-events.ts region=example
```

The events and their fields are listed in [Events](../reference/events.md). A channel marked
`noLog: true` is masked in all of them.

## Traces and costs

`exportTracesToOtlp` sends one trace per run, with a span per node, to any OpenTelemetry
(OTLP/HTTP) collector: Jaeger, Grafana Tempo, Honeycomb, Langfuse, LangSmith and others.

```ts file=packages/graph-sdk/examples/docs/observability-otlp.ts region=example
```

It traces the runs of that `app` (`run`, `resume`, `approveAndResume`, `signal`). For the
catalog runner, pass `onEvent` to `runCatalogGraph` and forward the events to your tracer.

Agent spans carry token usage and an estimated cost (`ailu.cost.usd`). The estimate uses a
built-in price list; pass `priceBook` to use your own prices. `computeCost(usage, model)` gives
the same estimate for one agent result's `usage`.

## Watch a run in the browser

`serveInspector(app, input)` runs the graph once and serves a live view of it on
`http://127.0.0.1:4517`: nodes light up as they run, and a suspended run shows what it waits
for. It is a development tool and listens on the local machine only.

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=inspector
```

## Debug a run

**Where is it stuck?** `app.explain(runId)` returns the run's status, the node it stopped at,
why, and the call that continues it. For a state you saved yourself, use `explainRun(state)`.

**Why did it fail?** A failed run has `status: "failed"` and emits `run_failed` with the error.
A node that throws is retried when it has a `retryPolicy`; each attempt emits `node_failed`.

**Common errors:**

| Error | Fix |
| --- | --- |
| `no API key for provider '...'` | Set the variable it names, or `AILU_LLM_MOCK=1` to run offline. |
| `RustEngineRequiredError` | The native engine didn't load. See [Install](../install.md#requirements). |
| `ResumeStateNotFoundError` | `resume` was called on another `CompiledGraph`, or after a restart. See [Long-running runs](./long-running.md). |
| `GraphCompileError` | The graph is invalid. The message lists each problem with its code. |

Every SDK error has a `code`, a `hint` with the fix, and a link to its entry in
[Errors](../reference/errors.md).

## Next

- [Deploy to production](./production.md).
