# 0008 — PII detection, redaction & anonymization (GDPR / AI Act)

- Status: **Accepted** (phase 1 = control plane — DONE; phase 2 = engine seam, Rust + TS — DONE)
- Date: 2026-06-20
- Deciders: Mathieu (owner)
- Relates to: [0005 multi-provider gateway](0005-multi-provider-llm-gateway.md), [0006 sovereign deployment & KB permissions](0006-sovereign-deployment-and-kb-permissions.md), [0007 connectors](0007-tool-connectors-oauth-mcp.md)

## Context

Under GDPR / the EU AI Act, an institutional-memory platform must not leak personal data to
LLM providers. We want to **detect** personal data, **anonymize it before it reaches a model**,
and **alert / block** on leaks — configurable per knowledge base, governed and audited like
everything else. The best detectors (Microsoft **Presidio** + **GLiNER-PII**, or an
OpenAI privacy/moderation pass) are Python and heavy; they must stay **out of the OSS engine**.

## Decision

A **`PiiRedactor` seam** is the single concept. The engine owns only the seam (a trait/interface
with a **no-op default**); the heavy detection lives in a **self-hosted Presidio+GLiNER service**
the control plane calls over HTTP (OpenAI Privacy Filter = an alternative backend, cloud option).
Behaviour is a **per-namespace policy** mirroring the LLM router policy.

### Per-namespace policy (mirrors the router)

`kb_namespace_pii_policy` table (one row per namespace), owner-only to set:
- `level`: `off | detect | redact | block`
- `entities`: which entity types to watch (e.g. `EMAIL`, `PHONE`, `PERSON`, `IBAN`, `CREDIT_CARD`)
- `threshold`: minimum detector confidence (0–1)

Resolution + write mirror `RouterService.resolve/setPolicy` (`ensureNamespaceAccess`, upsert,
`@Roles("owner")`).

### The seam + hook

- **Engine (OSS, phase 2):** `PiiRedactor` trait in `llm-gateway` (Rust + TS), default no-op. A
  `RedactingLlmGateway` wraps `complete()`: redact `LlmRequest` before the provider call, hydrate
  `LlmResponse` after. Covers every LLM call on both the native (Rust) and TS paths.
- **Control plane (phase 1, now):** an HTTP-backed `PiiRedactor` that calls the Presidio+GLiNER
  service. Wired where the control plane already mediates: **KB ingestion** (anonymize on store)
  and **run input/output**. A regex fallback runs when no service is configured (dev works).

### Reversibility (vault)

Redaction replaces a span with a stable placeholder (`[EMAIL_1]`) and stores the
placeholder↔value mapping in an **ephemeral, encrypted vault** (AES-256-GCM, reusing
`connectors.crypto`), scoped to one operation and cleared on completion. Outputs are
re-hydrated from the vault so the user sees real values while the model never did.

### Governance & audit

- Events `pii_detected` / `pii_redacted` / `pii_blocked` are written to the run journal
  (`events.type` is a free string → phase-1 needs no engine change; the typed union gains them
  in `contracts/events.ts`).
- `level: block` raises the existing approval gate (phase 2: `DynamicInterrupt` +
  `approval-engine`); phase 1 refuses the run and emits `pii_blocked`.
- The EU AI Act report (`ComplianceRunReportDto`) gains a `piiEvents` section.

## Security

- Detection deps (Presidio/GLiNER) never enter the OSS engine — separate self-hosted service.
- The vault holds sensitive data: encrypted at rest, **ephemeral** (never persisted plaintext),
  key from env (`CONNECTOR_ENC_KEY` family).
- Policy is owner-only and tenant/namespace scoped (`ensureNamespaceAccess`).
- The redactor service is called over HTTP only (no subprocess) — same posture as MCP-inbound.

## Alternatives considered

- **Redact only at the control-plane run boundary** (no per-LLM-call seam): misses intermediate
  agent messages / tool outputs sent back to the model. Rejected as the end state; acceptable as
  phase 1 (combined with KB-ingest redaction).
- **Bake Presidio into the engine**: violates OSS-light + Rust/Python boundary. Rejected.
- **Client-side redaction in the Studio**: unenforceable + bypassable. Rejected.

## Reservations

- **Latency**: an extra HTTP hop per redacted payload — cache + batch; `off`/`detect` levels skip
  the redact path.
- **False positives**: tune `threshold` + `entities` per namespace; `detect` mode (log only)
  before enabling `redact`.
- **Re-hydration fidelity**: placeholders must round-trip exactly; the vault is the source of
  truth, validated end to end.
- **Vault security**: encrypted + ephemeral; never logged.

## Phasing

1. **Phase 1 (control plane)** — policy table + endpoint (owner-only), `PiiRedactor` HTTP client +
   regex fallback, ephemeral encrypted vault, redaction at KB ingest + run I/O, `pii_*` events,
   compliance section, Studio policy panel. Reference Presidio+GLiNER service spec (FastAPI +
   compose).
2. **Phase 2 (engine seam) — DONE.** `PiiRedactor` trait + `RedactingGateway` wrapper in **both**
   gateways: TS (`llm-gateway/src/redacting-gateway.ts`, no-op default) and **Rust**
   (`crates/llm-gateway/src/redactor.rs` — `PiiRedactor` async trait, `NoopPiiRedactor`,
   `RedactingGateway`, and a generic `HttpPiiRedactor`). The Rust napi bridge wraps the gateway
   at agent-build time (`bridge.rs::wrap_with_redactor`) when `AILU_PII_REDACTOR_URL` is set,
   so **every native intermediate LLM call** (tool observations, prior turns) is scrubbed before a
   provider sees it — closing the gap that input/output redaction at the control plane can't reach.

   **Approach A (Rust → control plane):** the engine POSTs outbound texts to the control plane's
   `POST /pii/redact-batch` (`{ texts } -> { texts }`, same order), which runs `redactOutbound`
   against the `default` namespace policy (`off`/`detect` pass through; `redact`/`block` strip the
   spans). Policy + detection stay single-sourced in the control plane — no Presidio client or vault
   duplicated in Rust. The endpoint is `@Public()` + guarded by `AILU_PII_REDACTOR_TOKEN` (the
   engine sends it as a bearer; unset → open for loopback/dev).

   **Hydration** of the final answer stays at the control-plane run I/O path (it owns the per-run
   vault); the gateway seam only redacts (`hydrate_response` = identity), so there is no
   cross-boundary vault problem. The stored artifact remains anonymized (data minimization at rest).

   **Env:** `AILU_PII_REDACTOR_URL` (engine → control-plane batch endpoint) + optional
   `AILU_PII_REDACTOR_TOKEN`. Distinct from the control plane's own `PII_REDACTOR_URL` (which
   points at a Presidio `/detect` service) so the two never collide.

   **Block is fail-closed.** The batch endpoint returns `{ texts, blocked }`; on a `block`-level
   match the Rust seam returns `LlmError::PiiBlocked` and the agent node surfaces an error result
   instead of an answer (the agent node catches it — a blocked agent does not crash the graph; the
   PII never reaches a provider). `redact`-level scrubs and continues. A transport error to the
   redaction service is fail-OPEN (the hard block lives at the input gate; a flaky service must not
   abort a valid run). The remaining refinement is `block` as a true **human gate-and-resume**
   mid-loop (suspend → approve), vs the current stop-with-error.

   **Runtime activation:** the seam is compiled into the napi addon, so taking it live requires a
   **napi rebuild** (`napi build`/per-platform artifacts).
