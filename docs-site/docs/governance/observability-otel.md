---
sidebar_position: 2
title: Observability — OpenTelemetry & cost
description: Export a run as OTLP traces to LangSmith / Langfuse / Phoenix, with per-run token usage and cost.
---

# Observability — OpenTelemetry & cost

Ailu turns a run's lifecycle into **OpenTelemetry traces** and reports **token usage + cost** —
so you can watch runs and attribute spend in the dev tool you already use. It is a read view over
the same `RunEvent` journal that forms the [audit trail](/docs/governance/observable-runs): the
exporter never alters a run, and a trace can never diverge from the audit truth.

## Token usage on the result

Every agent reports the tokens it spent on `AgentResult.usage` — summed across its ReAct loop
(prompt + completion + cache tokens):

```ts
const result = await app.run({ ... });
const agent = result.channels.agentResult; // AgentResult
console.log(agent.usage); // { promptTokens, completionTokens, cacheReadTokens?, cacheWriteTokens? }
```

## Cost

Map usage to dollars with a price book ($/1M tokens). Ailu ships an indicative
`DEFAULT_PRICE_BOOK`; supply your own to override (prices drift):

```ts
import { computeCost, DEFAULT_PRICE_BOOK } from "@ailu-ai/graph-sdk";

const usd = computeCost(agent.usage!, "claude-opus-4-8"); // uses DEFAULT_PRICE_BOOK
const usdCustom = computeCost(agent.usage!, "my-model", { "my-model": { inPerMtok: 2, outPerMtok: 8 } });
```

An unknown model costs `0` — never a guess.

## Export traces (OTLP → Langfuse / Phoenix / …)

`exportTracesToOtlp` subscribes to a graph's events and ships each run as an OTLP/HTTP-JSON trace:
a root `run` span plus one span per node, with `gen_ai.usage.*` + `ailu.cost.usd` on agent
spans. OTLP is vendor-neutral, so the same call feeds **LangSmith, Langfuse, Phoenix, Datadog,
Grafana, Honeycomb** — anything that speaks OTLP.

```ts
import { createGraph, exportTracesToOtlp } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "support" }).agentNode("triage", { llm, prompt: { system: "…" } }).compile();

// Endpoint from the arg or the AILU_OTEL_EXPORTER_URL env var. Fail-open: an export error
// never fails the run; no endpoint = no-op.
const stop = exportTracesToOtlp(app, {
  endpoint: "https://api.langsmith.com/otel/v1/traces",
  headers: { "Authorization": Basic process.env.LANGFUSE_API_KEY ?? "" }
});

await app.run({ ... });
stop(); // flushes + unsubscribes (also auto-flushes on run_completed / run_failed)
```

### Plug a dev tool

| Tool | Endpoint | Auth header |
| --- | --- | --- |
| **LangSmith** | `https://api.smith.langchain.com/otel/v1/traces` | `x-api-key: <LANGSMITH_API_KEY>` |
| **Langfuse** | `https://cloud.langfuse.com/api/public/otel/v1/traces` | `Authorization: Basic <base64(public:secret)>` |
| **Phoenix** (local) | `http://localhost:6006/v1/traces` | — |
| **Grafana / OTel Collector** | your collector's OTLP/HTTP traces URL | per your collector |

(Check each vendor's current OTLP endpoint; the shapes above are the common form.)

## Debug a run live

For local tracing without a collector, the `RunEvent` journal works on both engines:

```ts
const off = app.onEvent((e) => console.log(e.type, "nodeId" in e ? e.nodeId : ""));
for await (const ev of app.stream({ ... }, "debug")) console.log(ev); // every raw event, incrementally
```

See [observable runs](/docs/governance/observable-runs) for the event vocabulary and
[events & streams](/docs/reference/events-and-streams) for the stream modes.

## What's not here yet

Per-**token** streaming spans (token-by-token) are a follow-up (deep-agent phase 13). Today a span
covers a whole node; the agent's total usage + cost are on its span.
