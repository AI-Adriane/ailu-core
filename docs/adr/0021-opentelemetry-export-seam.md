# ADR 0021 — OpenTelemetry export as a feature-gated seam over the observability interface

- Status: Proposed (design + plan; implementation is a contained follow-up)
- Date: 2026-06-22
- Deciders: Mathieu (owner)

## Context

The engine already has a first-class observability layer (`crates/observability`): a `Tracer`
trait (`start_span` / `end_span` / `get_span` / `get_trace`), a `MetricCollector`, and an
`ObservabilityBus`, with **in-memory** impls (`InMemoryTracer`, …). The runtime emits a span
per node lifecycle. What's missing is exporting those spans/metrics to a real backend
(Jaeger, Tempo, Grafana, Datadog, …) — i.e. **OpenTelemetry**.

Two forces shape the decision:
1. The OSS engine must stay **lean** — heavy detection/ML/transport deps live behind seams
   (PII → external service, LLMLingua → external service). The OpenTelemetry SDK + OTLP/gRPC
   stack (`opentelemetry`, `opentelemetry-otlp`, `tonic`) is heavy and must not be a default dep.
2. The `Tracer` trait is **synchronous** (`fn end_span(...)`), so network export cannot block
   the run thread.

## Decision

Add OTel export as a **feature-gated decorator over the existing `Tracer` interface**, not a
rewrite:

- A cargo feature **`otel`** (off by default — the lean engine pulls nothing extra).
- An **`OtelTracer<T: Tracer>`** decorator: delegates to the inner tracer and, on `end_span`,
  hands the completed span to a **background sender thread** (`std::sync::mpsc` + `std::thread`,
  no tokio) that batches and POSTs.
- Transport = **OTLP/HTTP-JSON** (not gRPC), via `reqwest` (already a workspace dep) under the
  `otel` feature — avoids `tonic`/protobuf. Endpoint from `AILU_OTEL_ENDPOINT`
  (`…/v1/traces`), headers from `AILU_OTEL_HEADERS`.
- A **pure** `span_to_otlp(span) -> Value` mapping (unit-tested) building the
  `resourceSpans → scopeSpans → spans` shape (`traceId`/`spanId`, `name`, time, attributes,
  status, `run_id`/`node_id` as attributes).

### Two small enabling changes
- **Epoch on spans.** OTLP needs `startTimeUnixNano`/`endTimeUnixNano`; `Span` currently stores
  RFC-3339 *strings* only. Add `started_at_unix_nano: u64` (+ optional end) set by the tracer
  from `SystemTime` — additive, no parse/`chrono` dep.
- **Id encoding.** OTLP wants hex trace/span ids (16/8 bytes). Map our string ids to hex
  (pad/truncate to the required width) in `span_to_otlp`.

The bridge wires `OtelTracer` only when `AILU_OTEL_ENDPOINT` is set (mirrors the PII /
LLMLingua env-gated seams); unset → the bare in-memory tracer (default inert).

## Consequences

- OTel is **opt-in + lean**: default builds pull nothing; `--features otel` + the env var turn
  on export with no gRPC stack.
- Spans flow to any OTLP/HTTP collector (Tempo, Jaeger, Grafana Cloud, Datadog OTLP, …) — the
  engine becomes observable in standard tooling without bespoke wiring.
- The sync `Tracer` trait stays untouched; export happens off-thread (no run-latency hit).

## Reserves / plan

- Metrics export (`MetricCollector` → OTLP metrics) is a second step; this ADR does traces first.
- Live verification needs a collector (`docker run otel/opentelemetry-collector`) — CI can assert
  the `span_to_otlp` mapping (pure) + a POST to a mock; full e2e is manual.
- Implementation order: (1) `unix_nano` on Span + tracer, (2) pure `span_to_otlp` + test,
  (3) `OtelTracer` decorator + background sender behind `feature = "otel"`, (4) bridge env wiring.
