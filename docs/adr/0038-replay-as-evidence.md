# ADR 0038 — Replay-as-evidence: deterministic governed-run replay cross-checked against the attestation chain

- **Status:** Accepted (owner sign-off 2026-06-25)
- **Date:** 2026-06-25
- **Relates to:** [0037 product-engine-consumption](./0037-product-engine-consumption.md) (the engine door this builds on), the durable attestation chain (control-plane, private PR #30), [0033 token-streaming](./0033-token-streaming-and-subagent-tagging.md) (event-stream invariants)

## Decisions at sign-off (2026-06-25)

The owner accepted the ADR and resolved its blocking questions:

1. **Engine that backs the work: the PUBLIC `ailu-engine` (source of truth).** The Rust/napi/SDK changes land in the public repo, not the vendored private `engine/` tree. Consequence: the product cannot consume replay end-to-end until the engine is published to npm + repointed (`^1.4.0`) — which is money-blocked (Track D step 8). Replay is built + tested in the public engine first; product wiring (`verify-replay` endpoint) follows the repoint. (Reconciles with [0037](./0037-product-engine-consumption.md).)
2. **Record-and-replay scope: full LLM I/O per run.** The decision journal records the complete LLM request/response stream (plus clock readings + human grants), not just a deterministic skeleton with a pinned transcript. This is the only way to faithfully reproduce an agent's decisions on replay; the heavier per-run journal store is accepted.
3. **Equivalence relation, attested-field exclusion, two-property framing, standalone scope** — accepted as proposed below (semantic equivalence over the ordered `{status, subject}` decision set; `approvalId`/`decidedAt`/`resolvedBy` excluded as non-reproducible human/control-plane facts; `verifyChain` = tamper-evidence and `verify-replay` = faithfulness reported independently; this ADR stands alone alongside 0037).

## Context

The product's competitive moat is a **governed, resumable runtime where replay is _proof_, not just debugging** — "they attest + block; we run + resume + replay." A code-grounded audit of the current substrate (workflow `wf_77e939ad-a75`) found this claim is **unsupported on the production path today**:

1. **A replay primitive exists but is unreachable in production.** `GraphRuntime.replayFrom(runId, checkpointId)` (TS, `packages/graph-runtime/src/runtime.ts:565`) and `GraphRuntime::replay_from` (Rust, `crates/graph-runtime/src/runtime.rs:558/581`, with deterministic fork ids + passing unit tests) both fork a new run from a checkpoint and re-execute. The product even exposes `POST /runs/:id/replay → RunsService.replayFrom()`. **But** `replayFrom()` uses `createRuntime()` — the in-process TS runtime whose tool/action nodes are registered as no-op stubs (`async () => ({})`) — **not** the Rust catalog engine (`runCatalogGraph`) that executes real production runs. Replaying a real run re-runs empty handlers and reproduces nothing.
2. **The Rust replay is not exposed through the production door.** The napi `Entry` enum is `Start | Resume | Approve | Signal` (`crates/bindings/src/bridge.rs`) — no `Replay`. `lib.rs` exports no `engine_replay`; `graph-sdk`'s `NativeEngine` / `run-catalog-graph` have no replay surface.
3. **The runtime is non-deterministic and has no decision journal.** `now_string()` reads `SystemTime::now()` directly for `state.updated_at` and every event timestamp; LLM calls are live; approval ids/timestamps use `Date.now()+Math.random()` / `new Date()`. The word "journal" appears only in doc-comments — no journal type/module exists.
4. **The attestation chain binds fields a re-run cannot reproduce.** `attest()` signs the canonical view `{approvalId, runId, status, resolvedBy, subject:'tool:<name>', decidedAt}` (Ed25519, hash-chained by `prevHash`). Of these, `status` + `subject` **are** engine-derivable on a re-run (agent gated-tool decisions surface as `tool:<name>` subjects), but `approvalId` (random), `decidedAt` (wall-clock), and `resolvedBy` (the human) are control-plane/human facts a re-execution can **never** reproduce.

So "replay reproduces the exact attested bytes" is **not achievable** on the current substrate. The realistic, honest design is a **semantic equivalence** check, plus the structural work to make replay deterministic in the first place.

## Decision

**Add deterministic replay to the production engine** (it does not exist there today) and define **replay-as-evidence as a semantic equivalence check**, kept distinct from the existing tamper-evidence check:

1. **Decision journal** — at original-run time, record the non-deterministic inputs per run: LLM responses, clock readings, and human grants (`__approvedTools`). Additive to the existing checkpoint-after-every-node contract; checkpoints already persist durably (`PgCheckpointer`).
2. **Injected-clock seam** — replace `now_string()`'s direct `SystemTime::now()` with an injected clock. Live runs keep the real clock; replay feeds the journaled readings. Normal-run behaviour is unchanged.
3. **Production replay entry** — add `Entry::Replay` to the napi bridge + `engine_replay` in `lib.rs` routing to the existing `GraphRuntime::replay_from`; add `replayCatalogGraph` to `graph-sdk` + `engineReplay` on `NativeEngine`; route `RunsService.replayFrom()` to the catalog path when `isCatalogGraph && rustEngineAvailable` so replay exercises real tool/agent behaviour, not the TS stubs. Replay is **checkpoint-segmented**: replay up to the gate, then replay the resume with the recorded grant.
4. **Cross-check endpoint** — `POST /runs/:runId/verify-replay`: replay journal-fed, extract the ordered `{status, subject:'tool:<name>'}` decision set from the replayed run's pending/granted approvals, and assert each attested record's `(status, subject)` is reproduced **in order**.

**Two distinct, independently-reported guarantees:**
- `verifyChain()` (already shipped) — **tamper-evidence**: hashes intact, signatures valid, `prevHash` linkage sound. Proves the records were not altered.
- `verify-replay` (this ADR) — **faithfulness**: the signed decisions are the decisions the run would make again.

Claims stay honest: "tamper-evident chain" + "replay reproduces the same governed decisions" — never a single overstated "provably correct" guarantee.

## Invariant impact

- **Determinism** — touched directly. The injected-clock seam + decision journal are added to _make_ replay deterministic; they must not change normal-run behaviour (live runs keep the real clock/LLM; the journal only records).
- **Checkpoint** — extended, not weakened. The journal is additive to the existing per-node checkpoint contract.
- **Event** — replay forks a new run id (`create_fork_run_id`), so its lifecycle events are segregated and never pollute the original run's stream. The event vocabulary + human-gate suspend/resume semantics in `.cursor/rules/040-runtime.mdc` are preserved on the replay path.

## Public API impact

Multiple engine public-API surfaces change (⇒ mandatory human review):
1. **napi ABI** — new `Entry::Replay` variant + `engine_replay` export.
2. **graph-sdk** — `replayCatalogGraph` + `engineReplay` on `NativeEngine`.
3. **Rust seam** — injected clock replacing `now_string()`'s direct `SystemTime::now()`.
4. **control plane** — new `POST /runs/:runId/verify-replay` endpoint (+ a contracts DTO for its result).

## Alternatives rejected

- **Byte-identical replay** (reproduce the exact attested bytes incl. `approvalId`/`decidedAt`/`resolvedBy`) — impossible: those are random/wall-clock/human facts. Rejected in favour of semantic equivalence over `{status, subject}`.
- **Wire up the existing `replayFrom()` as-is** — it routes to the TS stub runtime (no-op tool/action nodes) and a non-deterministic substrate, so it proves nothing on a real run. Rejected; deterministic replay must be _added_.
- **Fold replay into `verifyChain()`** — conflates two different properties (tamper-evidence vs faithfulness). Rejected; kept as two independent checks.

## Open questions (RESOLVED at sign-off — see "Decisions at sign-off" above)

1. **Equivalence definition** — is "the run reproduces the same ordered `{status, tool-subject}` decisions" the right proof claim for buyers/auditors, or do you want a stronger/different relation? (product/compliance decision, not just engineering)
2. **Record-and-replay scope** — record the full LLM I/O per run (new per-run journal store), or replay only the deterministic skeleton with the LLM transcript pinned from checkpoints/events? The former is heavier but is the only way to faithfully reproduce agent decisions.
3. **Which engine backs the product** — replay work must land in whatever the product actually links. The product currently links the **private** `engine/` tree (`workspace:*`), but the **public** `ailu-engine` (1.4.0) is the source of truth and the private one is périmé (missing phases 8-16). Build in public + sync, or build in the vendored private engine? This decides where the napi/Rust changes go. (See [0037](./0037-product-engine-consumption.md).)
4. **Non-reproducible attested fields** (`approvalId`/`decidedAt`/`resolvedBy`) — exclude from the equivalence relation, or treat the original run's recorded values as fixed replay inputs? Affects how the proof is worded.
5. **Two-property framing** — is "verifyChain = tamper-evidence; verify-replay = faithfulness" acceptable for the pitch, so "replay-as-proof" is not overstated as a single guarantee?
6. **Scope vs ADR 0037** — fold replay-as-evidence into the parked engine-consumption ADR, or keep it standalone (this doc)?

## Consequences

- The killer demo becomes real: `suspend → approve → resume → replay → prove`, two green guarantees side by side (chain not tampered + run reproduces its signed decisions) on one screen — the move attest-and-block competitors cannot make.
- This is the largest remaining MVP build (engine + napi + sdk + control plane). It is **money-free** but **sign-off-gated**, and depends on the attestation hardening (atomicity) landing first so replay verifies against a solid chain.
- Environment inputs (fs policy, tenant LLM keys, KB seeding) are re-supplied per call on the live path and are not snapshotted with the checkpoint today; the journal must also capture them, or replayed decisions can legitimately differ.
