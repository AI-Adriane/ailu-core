# ADR 0040 — verify-replay: gate-traversal replay + control-plane faithfulness check

- Status: **Accepted** (signed off 2026-06-25)
- Date: 2026-06-25
- Builds on: [ADR 0038 — replay-as-evidence](./0038-replay-as-evidence.md), [ADR 0037 — product/engine consumption](./0037-product-engine-consumption.md)
- Layer: L5 of the replay-as-evidence plan (L1–L4 shipped: PRs #80, #81, #82, #83)

## Context

ADR 0038 established the **faithfulness** guarantee: a deterministic replay must reproduce the
**same governed decisions** a run was attested for — distinct from the tamper-evidence the Ed25519
hash-chain gives. L1–L4 shipped the engine + graph-sdk surface: an injected `Clock`, record/replay
LLM gateways, the napi `Entry::Replay` bridge, and `replayCatalogGraph` + `verifyReplayDecisions`
in the SDK door (#83).

L5 is the control-plane endpoint that actually performs **record → replay → compare against the
attested chain**, in the private product. Grounding the product repo surfaced three structural
realities that shape the design:

1. **Catalog (Rust) runs persist only the terminal checkpoint.** `persistCatalogOutcome` saves a
   single checkpoint (`runId:version`) = the final/suspended state. There is no entry checkpoint
   and no intermediate ladder. `replayCatalogGraph` seeds a checkpoint and `replay_from`s it, so it
   has nothing to replay a *whole* run from.
2. **Gates do not self-traverse on replay.** A replay re-feeds the LLM journal, so the agent
   re-requests the same tool *subjects*. But a gate's **status** (approved/rejected) is a human
   input, absent from the LLM journal — so a replay from the entry state would **re-suspend** at the
   first gate instead of reproducing the rest of the decision sequence.
3. **The replay surface is unpublished.** Published `@ailu-ai/graph-sdk` is `1.4.3` (no replay
   symbols); the napi addon has no `engine_replay`. The product pins `^1.4.3`. L5 is blocked on an
   engine publish + a product dep bump (ADR 0037 cutover discipline).

## Decisions (signed off via AskUserQuestion, 2026-06-25)

- **D1 — Replay strategy: seed entry checkpoint + `replay_from`.** The control plane persists the
  **entry checkpoint** (initial `GraphState`) for recorded runs and replays forward from it via
  `replayCatalogGraph`. (The alternative — re-run from `initialData` — was not chosen.)
- **D2 — Record scope: always-on.** Every catalog run captures a replay journal and an entry
  checkpoint. Maximal coverage (every run is verifiable). Storage + sensitive-data surface are
  accepted; pruning is a follow-up.
- **D3 — Endpoint result: both guarantees.** `POST /runs/:id/verify-replay` runs `verifyChain`
  (tamper-evidence) **and** the faithfulness check, returning both verdicts plus the decision diff.

## The engine increment (the part needing review)

> **Design converged through grounding (2026-06-25), in two steps.**
>
> *Step 1* — the first draft put approval resolutions *into the replay journal* and had the engine
> auto-resolve gates from it. Rejected: ADR 0038 deliberately keeps human grants **out** of the
> journal (the journal is LLM I/O + clock — deterministically reproducible facts; grants are
> control-plane facts). Journaling them duplicates the attestation chain → a **circular** comparison.
>
> *Step 2* — the next design pre-granted the chain's approved tools and collected the replayed
> subjects via a hook in the agent middleware. Rejected as over-invasive: `RunCtx` is a deliberately
> minimal read-only per-hook snapshot, and the middleware is a public API surface (ADR 0025);
> threading a run-level sink through it for a replay-only signal is disproportionate. It also forced
> a new replay-mode gate behavior (a runtime invariant change).
>
> **The converged design needs no invariant change and no journal change** — a *single additive
> output field*. It leans on a fact already true of the engine: a replay from the entry state, with
> the LLM journal re-fed, **re-suspends at the first gate** and surfaces the agent's requested
> subjects in the **existing** `RunOutcome.pending_approvals`.

The engine increment is **exactly one additive change**:

- **Entry-state surfacing.** `runCatalogGraph` (record mode) surfaces the **entry** `GraphState`
  (the initial state, before the entry node runs) so the control plane can persist it as the
  checkpoint `replay_from` seeds from. `RunOutcome.entry_state: Option<GraphState>` (skip-if-None);
  the engine builds this state internally at run start — it is just not returned today. **No
  invariant touched** (live suspend/gate behavior, the journal, `crates/llm-gateway`, and
  `agents-core` middleware are all unchanged).

Everything else is **control-plane + the existing `Entry::Replay`**:

- verify-replay replays from the entry state (re-feeding the pure journal) → the run re-derives
  deterministically and **re-suspends at the first gate**, exposing the agent's requested subjects
  in `pending_approvals` (or completes, with none, for an ungated run).
- The control plane compares those **ordered requested subjects** to the attested chain's ordered
  subjects. The faithfulness signal is the **subject** sequence — what the agent requested, driven
  only by the journaled LLM outputs. The **status** is the human's decision, already covered by
  `verifyChain` (tamper-evidence).

This ships in the **public engine repo** as its own PR and is published as a new version
(e.g. `1.5.0`): bump `graph-sdk` + `ailu-cli` + `crates/bindings` + `python/pyproject` to the
tag (the `verify-tag` guard), Cargo workspace for the napi binary; confirm `model-core` is on npm at
a version `graph-sdk` resolves so `npm install` does not 404 (the 1.4.x cutover lesson).

### Scoped to v1; documented follow-ups

- **Multi-gate across sequential nodes.** v1 compares the **first** suspend's requested subjects.
  A run that gates at node A, is approved, resumes, then gates again at node B only surfaces A's
  gates in one replay pass. Full multi-gate re-derivation (pre-grant the chain's approvals into the
  entry state so the replay walks every gate in one pass, then collect per-gate subjects) is a
  follow-up — it needs the subject-collection mechanism Step 2 punted on.
- **Rejection replay** (a rejected decision's denied-branch re-execution is run-specific) — follow-up.

## The product L5 (after the engine publish + dep bump)

- **`run_journals` table** (Drizzle, mirrors `attestationsTable` conventions): `runId` (text, FK-ish,
  indexed), `tenantId` (text, nullable, indexed), `entryCheckpointId` (text), `entryState` (jsonb —
  the seed state for `replay_from`), `journal` (jsonb — the pure `{ decisions, clock }` blob),
  `createdAt` (timestamptz, defaultNow). `db:push`.
- **Record capture (always-on).** At catalog run start, persist the entry checkpoint. On completion,
  read `outcome.replayJournal` and persist it to `run_journals`. Record mode is enabled for catalog
  runs (no per-run flag — D2).
- **`POST /runs/:id/verify-replay`** (`@Roles('approver','owner')`, `RunScope`, `ensureRunInTenant`):
  fetch the attested chain (`getAttestationChain`) → load the entry state + pure journal from
  `run_journals` → `replayCatalogGraph(definition, entryState, entryCheckpointId, journal)` → read
  the replay's `pending_approvals` (the requested subjects, in order) → run `verifyChain`
  (tamper-evidence) **and** compare the replayed subjects to the chain's subjects in order →
  return `{ chainVerified, replayVerified, decisions: { attested, replayed, mismatches } }`.
  Contract DTO in `@ailu-ai/contracts`; Swagger-documented. (`verifyReplayDecisions` from #83
  drives the ordered comparison; the replayed `status` is `pending`, so the **subject** sequence is
  the discriminator — status faithfulness is `verifyChain`'s job.)
- **Local demo (no engine source build).** docker-compose Postgres + Redis, `db:push`, run a
  governed graph that suspends → approve → completes (recorded), then `POST .../verify-replay` shows
  both guarantees green. Consumes the **published** engine.

## Consequences

- **Sequence:** engine increment + publish (public) → product dep bump → product L5 endpoint + table
  + record capture + demo. L5 cannot land product-only; it reopens the engine — but only for a
  **single additive output field** (`entry_state`).
- **No invariant change.** Live suspend/human-gate behavior, the journal, `crates/llm-gateway`, and
  `agents-core` middleware are all untouched. verify-replay relies on the existing replay
  re-suspending at the first gate and surfacing `pending_approvals`. This is the key de-risking of
  the converged design.
- **Privacy/storage (D2 always-on):** the journal holds raw LLM outputs for every run. No
  pruning/retention yet — a documented follow-up, not a v1 blocker.
- **Scope:** v1 verifies the first gate's subjects + the chain's integrity. Multi-sequential-gate
  and rejection replay are documented follow-ups (above).
- **Read-only evidence:** verify-replay forks a read-only run (`<runId>:fork:<n>`), files no approval,
  opens no gate. The original run + its chain are untouched.

## Alternatives considered

- **Re-run from `initialData` (rejected at D1).** Avoids persisting an entry checkpoint but is a
  larger engine API and the same gate-traversal work; D1 chose checkpoint-seeding.
- **Journal-carries-approvals + auto-resolve from journal (rejected after grounding).** The first
  draft of this ADR; rejected because it duplicates the attestation chain into the journal (circular
  comparison) and violates ADR 0038's journal scope. See the *Refinement* note above.
- **Chain-driven step-by-step orchestration** (control plane resumes through each gate reading the
  attested decision). Rejected: a per-call `ReplayGateway` resets its journal cursor across the
  suspend→resume segments, re-feeding LLM calls from the start. The single-pass pre-grant design
  (decisions passed in, no suspend) keeps one journal cursor and one engine call.
- **Replay-only endpoint (rejected at D3).** Leaves tamper-evidence on the existing
  `GET /attestations`; D3 chose to return both in one response.
