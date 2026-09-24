# ADR 0004 — Approval enforcement on the Rust golden path (no self-approval)

- Status: Proposed
- Date: 2026-06-17
- Builds on: [ADR 0003](0003-ts-engine-deprecated-sdk-on-rust.md)

## Context

ADR 0003 flipped `@ailu/graph-sdk` onto the Rust engine and made the catalog run
path (`runCatalogGraph` / `resumeCatalogGraph`, driven by the control plane in
`apps/api`) the production "golden path" for graphs authored in the Studio. That path
ran agent nodes and human gates natively on Rust, but the **governance invariant was
not enforced on it**:

- `ailu_approval_engine`'s `ensure_can_resolve` (the no-self-approval check:
  `status == Pending` **and** `requested_by != resolved_by`) existed but was **never
  called from the bridge**.
- `bridge.rs`'s `drive` (the `Approve` branch) wrote every name in
  `approved_tools` into the `__approvedTools` channel **with no validation**.
- `EngineSpec.approved_tools` was a bare `Vec<String>` — it carried neither
  `requested_by` nor `resolved_by`, so the engine had nothing to validate against.
- On the control-plane catalog path, `usesApprovalEngine` was hard-coded `false`
  (`run-catalog-graph.ts`), so no `ApprovalEngine` request was ever filed for a
  suspended catalog run, and `resume` did not consult the engine before relaunching.
- Neither the TS nor the Rust path carried a `resolvedBy` principal end-to-end.

Net effect: a forged or buggy resume could unlock a sensitive tool without any human
of record approving it, including the agent "approving" its own request. The check we
already owned was simply not on the live path.

A note on ADR 0003's boundary contract: its "Architecture" section states the Rust
seam resolves each JS callback's **synchronous** return value and that a returned
thenable aborts the process. That description is **superseded** — Phase F moved the
seam to **async** callbacks: the Rust side now does
`call_async::<Promise<T>>(..).await?.await?`, awaiting the JS callback's returned
`Promise`. See "Rectification of ADR 0003" below.

## Decision

Enforce the no-self-approval invariant on the golden path with **defence in depth** —
two independent layers, neither trusting the other:

### 1. Control-plane authority (the source of truth)

`apps/api`'s `RunsService` owns whether a run may resume:

- **Emission.** `startCatalogRun` / `startGraphRun` pass `approvalEngine` to
  `runCatalogGraph`. When a catalog run suspends for approval, the SDK seam files one
  `ApprovalEngine` request per gated tool (`requestedBy = nodeId`, the agent's own
  subject) and stashes the engine request ids in the run's `__approvalIds` channel.
- **409 on pending.** `resume` calls `approvalEngine.getPending(runId)` **before**
  relaunching; if any approval for the run is still `pending`, it raises a
  `ConflictException` → **HTTP 409**. Resuming past an undecided gate is impossible.
- **Only approved tools.** `resumeCatalogRun` reads the stashed `__approvalIds`, looks
  each up, and unlocks **only** the tools whose approval the engine reports as
  `approved` with a distinct `resolvedBy`. Those names (sorted, de-duplicated) are
  written into `__approvedTools` before the resumed state is handed to the engine.
- `ApprovalsService.approve(id, resolvedBy)` stays the resolution point of truth; its
  `ensureCanResolve` (engine-enforced) is unchanged.

### 2. Engine guard-rail (defence in depth)

The Rust engine re-checks the invariant even though the control plane already did:

- `EngineSpec.approved_tools` becomes `Vec<ApprovedTool>` where
  `ApprovedTool { name, requested_by, resolved_by }` (`#[serde(default)]` for
  back-compat with start/resume specs that omit it).
- `bridge.rs`'s `drive` (`Approve` branch) validates each `ApprovedTool`: `resolved_by`
  must be non-empty **and** differ from `requested_by`, reusing
  `ailu_approval_engine`'s `ApprovalError::SelfApproval`. A violation returns a napi
  error that **interrupts the resume** — no tool name reaches `__approvedTools`. Only
  validated names are written, sorted, so the channel write is deterministic.

### 3. TS mirror + serialization

So both engines agree on the wire and the TS fallback enforces the same rule:

- `rust-engine.ts` serializes `approvedTools` as `{ name, requestedBy, resolvedBy }`
  objects (matching the new `EngineSpec`).
- `CompiledGraph.approveAndResume` gains `resolvedBy` on `ApproveAndResumeOptions`.
  On the **TS** path it approves the matching pending requests **through the
  `ApprovalEngine`** under `resolvedBy` (so the engine's own `ensureCanResolve` runs)
  **before** `runtime.resume`. On the **Rust** path it passes the
  `{ name, requestedBy, resolvedBy }` objects to the guard-rail.

## Rectification of ADR 0003

ADR 0003's "Architecture: the napi async + ThreadsafeFunction bridge" bullet describing
a **synchronous** boundary contract ("the Rust seam resolves each JS callback's
synchronous return value — it does not await a returned Promise; a returned thenable
aborts the process") is **out of date**. The live contract (Phase F, napi 2.16) is
**asynchronous**: the Rust seam awaits the JS callback's returned `Promise` via
`call_async::<Promise<T>>(..).await?.await?`. This ADR is the authority for the boundary
contract; ADR 0003 carries a pointer here.

## Consequences

- **Public API (napi / SDK).**
  - `EngineSpec.approved_tools` is now `Vec<ApprovedTool>` (`{ name, requestedBy,
    resolvedBy }`) instead of `Vec<String>`. `#[serde(default)]` keeps start/resume
    specs (which omit it) deserializing; an approve spec MUST now send objects. The
    native addon must be rebuilt (`pnpm napi:build`).
  - `RustGraphRunner.approveAndResume` takes `ApprovedToolWire[]` instead of
    `string[]`. `ApproveAndResumeOptions` gains an optional `resolvedBy` (defaults to
    `"human"`). New SDK exports: `AgentApprovalBinding`, `toAgentApprovalBinding`.
  - `runCatalogGraph` / `resumeCatalogGraph` options gain an optional `approvalEngine`.
- **HTTP.** `POST /runs/:id/resume` returns **409 Conflict** when the run has any
  pending approval. Clients must resolve approvals first.
- **Security.** An agent can never unlock its own tool: a self-approved (or
  unresolved) `ApprovedTool` aborts the resume in the engine, and the control plane
  refuses to forward it in the first place. The two layers are independent.
- **Back-compat.** Start and resume paths are unchanged on the wire (they never sent
  `approvedTools`). Only the approve path's payload shape changed.
