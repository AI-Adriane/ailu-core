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

Agent spans carry token usage and a cost, with a status that says how far it can be trusted.
`estimateCost(usage, model, priceBook?)` returns the same result for one agent result's `usage`:

| Status | Meaning |
| --- | --- |
| `known` | The model is in the price book and every token type has an explicit rate. |
| `estimated` | The cost is priced, but from an indicative rate (the built-in price list is) or with cache tokens counted at the input rate. |
| `unknown` | The model, the usage or the price is missing or invalid. `amount` is `null`, never `0`. |

The result also has `coverage` (the share of tokens that were priced, from 0 to 1),
`unpricedTokens`, the `currency` and the `reasons` the cost is not `known`. Pass `priceBook` to
use your own prices. A price entry may add `cacheReadPerMtok`, `cacheWritePerMtok`, `currency`,
`source` and `indicative`. A rate of `0` is a real free price; a model that is missing from the
book is unknown.

A span carries `ailu.cost.status`, `ailu.cost.currency`, `ailu.cost.coverage` and
`ailu.cost.unpriced_tokens`, plus `ailu.cost.amount` when the cost is not unknown. In USD it also
carries `ailu.cost.usd`. **An absent `ailu.cost.usd` means the cost is unknown, never that the
call was free.** The run's root span carries the total as `ailu.run.cost.status`, `.amount`,
`.currency`, `.coverage`, `.complete` and `.unpriced_tokens`, when at least one node reported usage.

`aggregateCosts(results)` sums several results the same way: the amount is the measured part
only, an unknown call adds nothing and lowers `coverage`, and `complete` is true only when every
token was priced, in one currency. Pass the results in the order the events were emitted, and
compare totals with a tolerance: float addition is not associative.

On a dashboard, sum only the measured part and always show the coverage next to the total:

```ts
const summary = aggregateCosts(results);
const label =
  summary.amount === null
    ? "cost unknown"
    : `${summary.complete ? "" : "at least "}${summary.amount.toFixed(4)} ${summary.currency} ` +
      `(${(summary.coverage * 100).toFixed(0)}% of tokens priced, ${summary.status})`;
```

## Watch a run in the browser

`serveInspector(app, input)` runs the graph once and serves a live view of it on
`http://127.0.0.1:4517`: nodes light up as they run, and a suspended run shows what it waits
for. It is a development tool and listens on the local machine only.

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=inspector
```

## Debug a run

**Where is it stuck?** `app.explain(runId)` returns the run's status, the node it stopped at,
why, and the call that continues it. For a state you saved yourself, use `explainRun(state)` —
the engine's account, also `ailu.explain_run(state)` in Python.

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
