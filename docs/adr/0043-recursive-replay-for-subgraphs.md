# ADR 0043 — Recursive replay-as-evidence for subgraph-bearing runs

- Status: **Shipped** — D1/D2/D3 + the backward-compat backfill (2026-08-09, PRs
  #198/#199/#200/#201/#203). Mathieu chose the full fix (Option A) over the cheaper, honest-gap
  alternative (Option B) proposed alongside it. Proven end-to-end by
  `subgraph-replay.rust.test.ts`: a parent + subgraph-child run recorded then replayed with
  `subgraphs` supplied reproduces BOTH the parent's and the child's LLM output exactly, and
  omitting `subgraphs` on replay still fails loudly (`SubgraphNotFound`).
  The backward-compat gap this status line previously flagged as open (a pre-fix `LlmJournal`
  degrading on replay — every `RecordedCall` deserializing with `run_id: None` while the new
  replay code tags reconstructed requests with `Some(<run_id>)`, so `ReplayGateway`'s
  request-equality match missed on EVERY call) is now closed by PR #203:
  `ReplayMode::resolve` backfills `run_id: None -> Some(top-level run id)` on every call in a
  loaded journal before constructing the `ReplayGateway`. Confirmed both by a unit test and an
  end-to-end empirical repro (record → strip `runId` → replay → now reproduces the exact
  recorded answer instead of degrading to an error).
- Date: 2026-08-07
- Deciders: Mathieu (owner)
- Follow-up to issue #184 (Tranche 2 of ailu#485/#512) — investigated and proposed via that
  issue's own comment thread before this ADR; see the issue for the full trace. Also corrects ADR
  0042 D1's own text (below).

## Context

Verified directly against the code, not assumed:

- **`replayCatalogGraph`** (`packages/graph-sdk/src/run-catalog-graph.ts:561`) never threads a
  `subgraphs` array — `RunCatalogGraphOptions`'s `Pick<...>` for it omits the field entirely. A
  subgraph-containing replay hits `RuntimeError::SubgraphNotFound` (`graph-runtime/src/
  runtime.rs:118/1002/1107`) — a generic "not found" error, not a purpose-built "replay doesn't
  support this" signal.
- **ADR 0042 D1's own text was wrong and never corrected**: *"`replayCatalogGraph` does not need
  [subgraphs], since a subgraph node's execution is itself journaled as part of the parent's
  replay just like any other node transition."* PR #173 (D1's own implementation) added the
  `SubgraphNotFound` guard instead — the assumption did not survive contact with the actual
  journal format. This ADR supersedes that specific claim.
- **The real blocker: `LlmJournal` is flat and untagged.** `llm-gateway/src/replay_journal.rs`:
  `LlmJournal { calls: Vec<RecordedCall> }`, and `ReplayGateway` matches "by request equality,
  consuming each recorded call once in occurrence order" (the module's own doc comment). No
  `RecordedCall` carries any run/child identifier. For a single flat run this is sufficient — one
  linear consumer. It cannot correctly partition a parent's calls from a child subgraph's calls,
  and REALLY cannot do so for `mapAgents`-style dynamic fan-out, where several children's calls
  interleave by real concurrent completion timing.
- **The child-run-id mechanism already exists and is deterministic** — this ADR does not need to
  invent one. `subgraph_run_id` (`graph-runtime/src/runtime.rs:220-226`):
  `RunId(format!("{}:{}", state.run_id.0, node_id.0))`. `execute_subgraph`
  (`runtime.rs:988-1028`) computes `child_run_id` this way and recurses into
  `start_with_ctx`/`resume_with_ctx` with it — so `state.run_id` inside `execute_node` is ALWAYS
  the id of whichever run (parent or child) is currently executing, correctly, throughout the
  existing recursion. Product-side memory independently confirms the same `{parentRunId}:
  {nodeId}` / `{parentRunId}:{nodeId}:{index}` convention is already used for child run ids
  elsewhere (`RunsService`) — this ADR reuses that exact scheme for dynamic fan-out cardinality
  too, rather than a second one.
- **This also answers product issue #485's open cardinality question** ("how does this interact
  with `mapSubgraph` fan-out, where children are dynamically many and not known until runtime?"):
  tagging by the SAME deterministic run-id scheme handles one child or N dynamically-spawned
  children uniformly — no special-casing fan-out vs. single-subgraph.

## Decision

### D1 — `LlmRequest.run_id: Option<String>`, set by the caller, recorded verbatim

Add `run_id: Option<String>` to `LlmRequest` (`types.rs`), additive/optional exactly like
`system`/`tools`/`response_format` already are. Whatever constructs an agent's `LlmRequest`
(agents-core's ReAct loop) sets it from the run's own `state.run_id` — already in scope there,
correctly parent or child, exactly as computed by `subgraph_run_id`/passed through
`start_with_ctx`/`resume_with_ctx`. No new id scheme; reuses the existing deterministic
convention. `RecordingGateway::complete` copies `request.run_id.clone()` onto the `RecordedCall`
it journals — no trait signature change, no provider file touched.

### D2 — `ReplayGateway` matches on `(run_id, request)`, not `request` alone

`ReplayGateway`'s existing "match by request equality, consuming each recorded call once in
occurrence order" extends to also require `call.run_id == request.run_id`. Since a replayed
child's own `LlmRequest`s carry the CHILD's `run_id` (set the same way as during recording — the
replay path re-executes through the same agent code), this needs no separate "construct
`ReplayGateway` scoped to a target run" step: the SAME shared `ReplayGateway`/journal correctly
serves the parent's calls to the parent and the child's calls to the child, because each
request's own `run_id` field discriminates them. A recursive replay of a subgraph-bearing run
therefore requires NO new partitioning logic beyond this one added equality check.

### D3 — `replayCatalogGraph` gains `subgraphs`, mirroring `runCatalogGraph`/`resumeCatalogGraph`

Once D1/D2 land, add `subgraphs?: GraphDefinition[]` to `replayCatalogGraph`'s options (closing the
literal ask of issue #184) — the SDK-level plumbing this issue originally scoped is now safe to add
because the journal underneath it can actually support it.

## Consequences

- **Backward compatibility for already-persisted journals — SHIPPED (PR #203)**: an `LlmJournal`
  recorded BEFORE this change has no `run_id` on its calls. `ReplayMode::resolve` backfills every
  `RecordedCall.run_id: None` to the TOP-level run's own id for historical journals (every
  pre-fix journal was single-run by construction — no subgraphs were ever replay-capable, so
  every recorded call legitimately belongs to the top-level run). A historical journal replayed
  under the NEW code behaves identically to before.
- **`verifyReplayDecisions`/`RunEvidenceView` (product side)**: once recursive replay is real,
  the "whole-tree `chainVerified`" work (Tranche 1, PR #519) and this recursive `replayVerified`
  need to agree on the SAME child-discovery mechanism (parent-run back-references) — not designed
  here, a product-side follow-up once this engine change lands.
- This is a `RecordedCall`/`LlmJournal` FORMAT change on a published package
  (`@ailu/llm-gateway` is Rust-internal via napi, but the journal's serialized shape is what
  the control plane persists) — needs the same version-bump/consumer-coordination discipline as
  any other engine release (release-unified-versioning precedent).

## Rejected alternatives

- **Option B (the honest-gap alternative, also proposed)**: replace `SubgraphNotFound` with a
  named `replayVerified: "not_supported_for_subgraphs"` outcome, zero journal changes. Cheaper,
  ships today, but never actually closes the gap — Mathieu chose to fix it for real instead.
- **A second, subgraph-specific id scheme** instead of reusing `subgraph_run_id`'s existing
  convention — rejected: the convention is already proven, already used product-side, and this
  ADR's whole point is that tagging by run id is enough; inventing a parallel scheme would just be
  two sources of truth for the same concept.

## Open question #1 — RESOLVED, corrected estimate (smaller than the first correction)

First pass: `ReplayMode::wrap_gateway` (`runtime-bridge/src/lib.rs:168-176`) wraps each agent's
bare gateway in a `RecordingGateway` sharing ONE journal `Arc` for the entire run — built at
agent/engine-spec construction, BEFORE any node executes, reused unchanged across a subgraph
child's recursion. `LlmGateway::complete`/`::stream` (`gateway.rs:25/50`) take no run-id
parameter. My first read concluded D1 needed a `LlmGateway` TRAIT signature change (`run_id: &RunId`
on `complete`/`stream`) — touching every implementor.

**That was an over-estimate — `LlmRequest` is the better seam, already has the exact precedent.**
`LlmRequest` (`types.rs:207-224`) already carries THREE additive, optional fields following this
identical pattern (`system`, `tools`, `response_format` — each `#[serde(default,
skip_serializing_if = "Option::is_none")]`, each read individually by providers, never required).
Adding `run_id: Option<String>` the same way needs **zero trait signature changes and zero
provider changes** — confirmed no implementor destructures `LlmRequest` exhaustively
(`grep "let LlmRequest {"` across the whole crate tree: no matches; every provider reads specific
fields via `request.messages`/`.system`/etc., e.g. `openai_compatible.rs:436-445`). The request
object is what `RecordingGateway`/`ReplayGateway` already receive whole — they just read
`request.run_id` directly. The only real changes: (a) `run_id: Option<String>` on `LlmRequest`,
(b) whatever constructs an agent's `LlmRequest` (agents-core, ReAct loop) sets it from the current
`state.run_id` it already has in scope, (c) `RecordingGateway::complete` copies it onto
`RecordedCall`, (d) `ReplayGateway::complete` filters the journal by it before matching. No trait
change, no provider-file changes at all.

## Open question #2

Should the `run_id` tag be a plain `String` (matching `RunId`'s own `.0` field, simplest) or a
richer struct that also disambiguates a fan-out child's `index` explicitly (rather than relying on
it being baked into the id string via `{parent}:{node}:{index}`)? Leaning toward plain `String`
(no new type, matches how `RunId` is already just a string wrapper) unless implementation reveals
a reason otherwise.
