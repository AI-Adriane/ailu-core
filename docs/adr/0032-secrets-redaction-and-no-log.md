# ADR 0032 — Secrets redaction + no-log channels (deep-agent phase 10)

- **Status**: Accepted (design signed off 2026-06-23; decisions D1–D5)
- **Context**: deep-agent phase 10 of [ADR 0023](0023-governed-deep-agent-platform-landscape.md). Mirrors the PII seam ([ADR 0008] / `RedactMiddleware`); governed-by-construction ([ADR 0013]/[ADR 0025]); deterministic runtime.

## Context

Phase 10 closes the last obvious governance leak. Two surfaces leak today:

1. **Outbound to providers.** The only governed redaction is PII — `PiiRedactor` (`crates/llm-gateway/src/redactor.rs:23-37`) wrapped by the governed `RedactMiddleware` (`crates/agents-core/src/middleware.rs:310-340`), installed only via the **env-gated** `HttpPiiRedactor::from_env()` at the single `push_governed` site (`crates/bindings/src/bridge.rs:338-339`), fail-**open** on transport error. So in the OSS default (no `AILU_PII_REDACTOR_URL`) **nothing** scrubs an outbound request — an API key in a prompt/tool-output ships verbatim. Secrets are a **closed, well-known pattern set** (`sk-`/`sk-proj-`, `AKIA…`+40-char AWS secret, `ghp_`/`gho_`/`ghs_`, `xox[baprs]-`, `AIza…`, `sk_live_`, `Bearer`/JWT `eyJ…`, PEM private-key headers) — no ML needed.
2. **Inbound to events/logs.** `RunEvent::NodeCompleted` is the only event carrying channel values (`crates/graph-runtime/src/types.rs:40-45`, `output: BTreeMap<String,Value>`); four emit sites pass raw channel data (`runtime.rs` normal/suspend ~757, fan-out ~804, subgraph ~922 — the child's whole channel map). Everything funnels through `InMemoryEventBus::emit` → the fire-and-forget napi `on_event` which `serde_json`-stringifies it → logs/UI. Crucially `persist_checkpoint` (`runtime.rs:990-1001`) saves full state on a **separate** path from events — durability and observability are already decoupled, which makes "checkpoint in full, mask only the event view" cleanly expressible.

Governed-by-construction: `push_governed` is builder-only + sealed; `request_order` chains governed→efficiency (`middleware.rs:185-187`); the bridge match routes governance kinds to `_ => {}` so spec/user data can never reach `push_governed`.

## Decision

Ship phase 10 as two mirrored, governed-by-construction seams.

### Seam A — Secrets redactor (outbound). Both layers (D1), mask-default + block opt-in (D2), one-way (D5), before PII (D4)
- **In-engine floor (always-on):** `RegexSecretsRedactor` (new, in `llm-gateway/redactor.rs`) implements the **existing `PiiRedactor` trait** (so it reuses `RedactMiddleware`-shaped machinery), with a **fixed, versioned, constant** regex set (no `eval`, no dynamic patterns — honours the security hard rule). It walks the same ordered text set as `HttpPiiRedactor::redact_request` (system + per-message Text `content_blocks` OR `content`, ADR 0030 traversal, same-order write-back) applying **local regex** — no transport, no env gate, offline, replay-stable. Matches → a **typed one-way placeholder** `[REDACTED:OPENAI_KEY]` / `[REDACTED:AWS_KEY]` / generic `[REDACTED_SECRET]` (carries the matched **class**, never the value or a hash). `after_model` is **strict identity** (secrets are never re-hydrated; no vault).
- **External augmentation (optional):** `HttpSecretsRedactor` (mirror of `HttpPiiRedactor`, env `AILU_SECRETS_REDACTOR_URL`) for org-specific secrets, installed as a second governed middleware when the URL is set. Defense-in-depth, **not** the only line.
- **Policy (D2):** **mask-and-continue is the default**; an **opt-in block** (env `AILU_SECRETS_POLICY=block`) makes the floor fail-closed via a new typed `LlmError::SecretsBlocked` (surfaced at the node sink as channel data, never a panic — the `PiiBlocked` precedent).
- **Order (D4):** the secrets floor is pushed **before** PII at the bridge governed-injection site, so it runs outermost on `before_model` and strips keys/tokens **even when PII is a no-op** and **before** any text reaches the external PII service. Both stay in the sealed governed layer; the secrets kind is **not** a nameable SDK middleware kind (engine-decided, unrepresentable to install/omit).

### Seam B — No-log channel marker (inbound). Typed flag, masked at events, checkpointed in full (D3)
- Add an additive `#[serde(default)] pub no_log: bool` to `ChannelDefinition` (`crates/graph-core/src/types.rs`), camelCase `noLog` over the wire, validated by `GraphValidator`, serde-default back-compatible (existing graphs deserialize unchanged), carried on the `GraphDefinition` and the contracts schema.
- A single DRY `mask_output(output, channels)` helper masks the **event view** at the four `NodeCompleted` emit sites: for each key, if its `ChannelDefinition.no_log` is set, replace the **value** with a deterministic sentinel `[REDACTED_NO_LOG]` (replace, not omit — streaming UIs still see the channel changed). The subgraph site consults the **child** graph's channel defs.
- **Durability rule (confirmed):** masking is a **view transform on an event-bound clone only**. `apply_update` + `persist_checkpoint` operate on **unmasked** values, so reducers fold identically and resume/time-travel produce byte-identical state. A no-log channel is **checkpointed in full (durable)** and **masked only in events/logs**. `durability ≠ observability` — an intentional, documented asymmetry.

## Alternatives considered
- **External secrets seam only (mirror PII)** — zero new deps, symmetric, but the OSS default leaks (fail-open contradicts a secrets posture). Kept as the optional augmentation layer, not the floor.
- **In-engine floor only** — misses org-specific tokens. Kept as the floor; external seam adds coverage (→ "both", D1).
- **Block-by-default** — strongest, but fails benign runs + requires the public-API `SecretsBlocked` variant always. Chosen as **opt-in** (D2).
- **No-log: reserved channel-name convention** — brittle/untyped/unvalidated. Rejected for a typed validated flag.
- **No-log: mask inside `EventBus::emit`** — emit has no `GraphDefinition` access; would thread channel metadata through the bus trait. The four-site helper keeps metadata where it lives.
- **Omit no-log keys vs sentinel** — omitting changes observable shape; sentinel preserves it.
- **Vault + hydrate secrets (mirror PII vault)** — secrets have no legitimate restore use + a vault is a stateful non-deterministic leak surface. Rejected (one-way, D5).

## Consequences
- **Determinism preserved**: masking is a view on an event-bound clone, never state; the regex set is fixed/versioned; placeholders are fixed strings (no random ids / value hashes) → replay-stable checkpoints + (masked) events.
- **Checkpoint vs event split is load-bearing + honest**: no-log channels are stored unmasked (resume/time-travel depend on it) and masked only in the emitted/logged view. The in-memory event log stores the masked view; checkpoints stay full-fidelity.
- **Governed-by-construction held**: the secrets floor is sealed + unconditional (`push_governed` only), kept out of every efficiency match arm; `no_log` is a typed validated field, not user free-data.
- **Order is structural**: governed→efficiency (redact-before-compress), secrets-before-PII within governed.
- **Public-API surface (mandatory review)**: new `RegexSecretsRedactor`/`HttpSecretsRedactor` + `SecretsRedactMiddleware` exports; new `ChannelDefinition.no_log`; new `LlmError::SecretsBlocked` variant (closed `PartialEq/Eq` enum). The TS `RunEvent` shape is unchanged.
- **Residual leak surfaces (acknowledged gap)**: `NodeFailed.error` / `RunFailed.error` / `RunSuspended.reason` are free-text a handler could interpolate a secret into — **not** covered here (candidate: a future emit-time string scrub).
- **Net-new dependency**: `regex` (already in the lock transitively via `jsonschema`) becomes a direct `llm-gateway` dep; the pattern set is engine-owned + versioned.

## Phasing
- **A1** — `RegexSecretsRedactor` (regex floor, mask/block mode) impl `PiiRedactor` in `llm-gateway/redactor.rs`; constant pattern set; unit tests (each class + same-order write-back + content_blocks). Export from `llm-gateway/lib.rs`.
- **A2** — `SecretsRedactMiddleware` (copy of `RedactMiddleware`, `name()=="secrets"`, identity `after_model`) in `agents-core/middleware.rs`; export from `agents-core/lib.rs`.
- **A3** — `LlmError::SecretsBlocked`; redactor block-mode returns it; bridge reads `AILU_SECRETS_POLICY`.
- **A4** — bridge: push the secrets floor **unconditionally before** PII; + optional `HttpSecretsRedactor::from_env()` second push. Test: present even with PII unset; precedes PII.
- **B1** — `ChannelDefinition.no_log` (+ validator + contracts schema + back-compat test).
- **B2** — `mask_output` helper + wire the four `NodeCompleted` emit sites (child graph at the subgraph site); tests: event masked, checkpoint unmasked, resume byte-identical.
- **C** — docs (governance page: secrets floor + order vs PII + checkpointed-but-masked rule + the residual free-text gap); ADR 0023 phase-10 status.
