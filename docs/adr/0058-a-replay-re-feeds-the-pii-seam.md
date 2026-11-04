# ADR 0058 — A replay re-feeds the PII seam

- Status: **Proposed**, for owner review. Prepared on the owner's go (2026-11-04); not merge approval.
- Date: 2026-11-04
- Decider: repository owner
- Relates to: [0038](./0038-replay-as-evidence.md) (the replay journal),
  [0008](./0008-pii-redaction-and-anonymization.md) (the PII seam),
  [0025](./0025-unified-agent-middleware-api.md) (the middleware stack),
  [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) and [0045](./0045-host-nodes-in-rust-and-sdk-parity.md)
  (other non-deterministic inputs the journal serves).

## Context

The agent's governed middleware runs the external PII seam (`AILU_PII_REDACTOR_URL`) before the
efficiency middleware, and `RecordingGateway` journals the request the provider receives: after
the seam. A replay rebuilt every request from the seam its own environment configured
(`HttpPiiRedactor::from_env()` in `build_agent_middleware`), so a replay without the seam (an
offline `@ailu-ai/verify`, `env -i`) rebuilt the unmasked request. `ReplayGateway` matches by
request equality, so the call missed the journal; the miss is channel data, the agent took another
path, and the capsule of a run with an approved tool read one decision mismatch (ailu P10, on
2.7.4 and 2.8.2). With the seam configured, or one that rewrites nothing, the replay matched.

Comparing the replayed request to the journaled one instead of rebuilding it does not work: the
efficiency middleware (skills, context budget, terse, compression) rewrites the request after the
seam, so the journaled request is not the seam's output, and request equality is the integrity
check of the replay.

## Decision

The seam is a non-deterministic input, served from the journal like the model, the clock, host
tools and host nodes.

1. **Record.** In record mode the seam is wrapped in `RecordingPiiRedactor`, which appends one
   `RecordedRedaction` per call to the run's journal: `runId`, `nodeId`, and `texts`, the seam's
   output for each redactable text of the request, `null` where it returned the text unchanged;
   or `blocked`, the reason, when it blocked the call. The texts the seam received are never
   written. The journal carries the list as `piiSeam`, only when the run called the seam.
2. **Replay.** A journal with `piiSeam` replaces the seam with `ReplayPiiRedactor`, whether or not
   the replay's environment configures one: it takes the next unconsumed output of the same
   `(runId, nodeId)` and writes it over the request's texts. It never calls a service.
3. **Fail closed.** No output for the call, or one recorded for a different number of texts, fails
   the call with `LlmError::ReplayJournalMiss`; its message names the run and node, no text. The
   request is never passed on unredacted. A recorded block replays as `PiiBlocked`.
4. **Old journals.** A journal without `piiSeam` (recorded before 2.8.3) replays as before: with
   the seam the environment configures. Without one, its calls miss the journal, typed, as today.

The key is `(runId, nodeId)` in order. An agent's calls are sequential; concurrent spawns and
subgraph children have their own run id (ADR 0043); parallel nodes have their own node id.

## Invariants

- **Determinism**: strengthened — the replay no longer depends on an external service.
  Live runs are unchanged; a record run adds one journal list.
- **Checkpoints, events, human gates**: unchanged.
- **Security**: the journal gains only text the seam returned, which the journaled requests
  already carry. No text reaches a provider on a replay (`ReplayGateway` has none).

## Consequences

- The fix lives where every path goes through: the Rust bridge's `ReplayMode` and the agent
  middleware stack. The TypeScript and Python SDKs replay through the engine and need no change.
- The external secrets redactor (`AILU_SECRETS_REDACTOR_URL`) and the compressor
  (`AILU_LLMLINGUA_URL`) are external services of the same kind and are not covered here.
- A text the seam rewrote is replayed from its recorded output, so the replay no longer checks
  the text the seam received at that position: only that the request has the same number of
  texts. Texts the seam left unchanged, and every other field, are still compared byte for byte by
  `ReplayGateway`. Checking the received text would need it, or a digest of it, in the journal:
  a digest of a short personal value can be confirmed by guessing, so neither is recorded.
- `@ailu-ai/verify` still notes `AILU_PII_REDACTOR_URL` as a service a replay may reach: true for
  an old journal, an over-warning for a new one.
