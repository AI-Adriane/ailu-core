# ADR 0028 — Observability: OTLP export, cost tracking, dev-tool integration (phase 7)

- Status: **Accepted** (GO 2026-06-23). Decisions adopted: OTLP-only exporter (vendor-neutral); `AgentResult.usage` = additive optional summed `LlmUsage`; `PriceBook` config/env-supplied with a default table; sequence **7a → 7b → 7c**. Implementation starts at 7a (engine usage + cost + spans-from-events, no external dep).
- Deep-agent platform: [ADR 0023](0023-governed-deep-agent-platform-landscape.md) **phase 7** (the capability audit's top gap).
- Builds on: the `observability` crate (`Tracer` / `MetricCollector` / `Span` / `Metric`, in-memory), the `EventBus` (`RunEvent` journal), [ADR 0014](0014-engine-token-efficiency.md) (`LlmUsage`), the seam pattern (PII / LLMLingua / fs-backend: env-gated external service, fail-open/closed).

## Context

Audit finding (2026-06-23): Ailu has a **span/metric abstraction** (`Tracer`, `MetricCollector`, `Span`, `Metric` + in-memory impls) but **no OTLP exporter** (nothing is sinked), token usage (`LlmUsage`, captured per LLM call incl. cache tokens) is **not surfaced** on `AgentResult`, there is **no cost mapping**, and the Monitor docs cover only the event journal. So a user cannot: see traces in a dev tool (LangSmith / Langfuse / Phoenix), attribute cost to a run, or set up OTel.

## Decision

Build observability as **derive-from-events + an env-gated OTLP exporter seam**, governed (spans carry the same `run_id`/`node_id` as the audit journal; the exporter is external, fail-open, and respects redaction). Three sub-phases:

### 7a — usage + cost on the engine (no external dependency)
- **`AgentResult.usage`** (additive, optional): the summed `LlmUsage` (prompt / completion / cache tokens) across the agent's iterations. The ReAct loop already sees each `LlmResponse.usage` — accumulate + attach. Wire-compatible (`#[serde(skip_serializing_if]`).
- **Spans from the run**: a `TracingObserver` subscribes to the `EventBus` and turns `node_started`→`start_span`, `node_completed`→`end_span` (status + the node output as attributes), so a run produces a span tree with **no new engine emission path** (the events already exist).
- **Cost**: a `PriceBook` (per `provider`/`model`, $/Mtok in/out, config- or env-supplied with a sensible default table) maps usage → a `cost` attribute on the agent span + a run-level metric. Prices change, so the book is data, not hardcoded.

### 7b — OTLP exporter seam (env-gated, external)
- An `AILU_OTEL_EXPORTER_URL` (OTLP/HTTP) seam: when set, spans + metrics are pushed to an **OTel collector**. **Fail-open** (export errors never fail the run — observability is best-effort, like LLMLingua). Reuses the `Tracer`/`MetricCollector` outputs.
- **Vendor-neutral by construction**: OTLP is consumed by **LangSmith, Langfuse, Phoenix, Datadog, Honeycomb, Grafana** — one seam, every dev tool. No vendor SDK in the engine.
- **Governed**: the exporter runs PII/secrets redaction (ADR 0008 / phase 10) over span attributes before egress — nothing sensitive leaves the perimeter unredacted; honours sovereign mode (ADR 0006).

### 7c — docs (Monitor section) + dev-tool how-to
- Document: trace setup (`AILU_OTEL_EXPORTER_URL`), the span/metric model, `run("debug")`/`onEvent`, cost via `AgentResult.usage` + the price book, and **plugging LangSmith / Langfuse** (point their OTLP endpoint at the engine). Fills the Monitor docs gap.

## Invariants
- **Audit ⊇ traces**: spans derive from the same `RunEvent`s that form the audit journal — observability is a read view, never a side channel that could diverge from the audit truth.
- **Fail-open egress**: a down/missing collector never fails or slows a run beyond a bounded timeout.
- **No unredacted egress**: span attributes pass redaction before export (tie-in with phase 10 secrets).
- **Determinism untouched**: tracing/metrics are observation only — they never alter execution or channel state.

## Open decisions (for sign-off)
1. **Exporter protocol** — OTLP/HTTP only (vendor-neutral, recommended) vs also a native LangSmith/Langfuse client? (Recommend OTLP-only: one seam covers all.)
2. **`AgentResult.usage` shape** — sum of `LlmUsage` (prompt/completion/cacheRead/cacheWrite) — confirm additive + optional (back-compat).
3. **PriceBook source** — env/config-supplied (recommended; prices drift) with a shipped default table, vs hardcoded.
4. **Sequencing** — 7a (engine usage+cost+spans-from-events) first, then 7b (exporter), then 7c (docs)? (Recommended — 7a is immediately useful + has no external dep.)

## Reserves / next
- Per-token streaming spans = phase 13 (token-level streaming). Studio's trace UI consumes these spans (a product view).
