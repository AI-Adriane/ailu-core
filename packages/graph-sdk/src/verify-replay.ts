//! Replay-as-evidence (ADR 0038): the FAITHFULNESS check — does a deterministic replay reproduce
//! the SAME governed decisions the run was attested for?
//!
//! This is deliberately SEPARATE from `verifyChain` (the tamper-evidence check over the Ed25519
//! hash-chained attestation). Two independent guarantees:
//!   - `verifyChain`        — the attested records were not altered (tamper-evidence).
//!   - `verifyReplayDecisions` — the signed decisions are the ones the run reproduces (faithfulness).
//!
//! A decision is its `{ status, subject }` (and its `callKey` when both sides name the call, ADR 0051
//! D3). The caller derives `subject` the SAME way the
//! attestation does (the description, else canonical JSON of the subject) for both sides, so this
//! helper is a pure ordered-equivalence over strings — it does NOT re-derive subjects or touch
//! crypto. `decidedAt` / `resolvedBy` / `approvalId` are intentionally NOT compared: they are
//! wall-clock / human / random facts a re-execution cannot (and should not) reproduce.
//!
//! The comparison is the engine's (`verify_replay_decisions`, ADR 0045 D3.4), the same for every
//! SDK; this module keeps the TypeScript types and signature.

import { engineVerifyReplayDecisions } from "./rust-engine.js";

/** One governance decision, reduced to what a replay can faithfully reproduce. */
export type ReplayDecision = {
  /** `"approved" | "rejected"`. */
  status: string;
  /** The decision subject, derived identically to the attestation (`description || canonicalJson`). */
  subject: string;
  /**
   * ADR 0051 D3 — the call the decision is about (`<name>#<sha256(canonical input)>`): the
   * attestation's `callKey` on one side, the pending approval's on the other. Compared only when
   * both sides carry one, so a record attested before ADR 0051 compares by subject as before.
   */
  callKey?: string;
};

/** The result of comparing the attested decisions to the replayed ones, in order. */
export type VerifyReplayResult = {
  /** True iff every attested decision is reproduced, in the same order, by the replay. */
  ok: boolean;
  attested: ReplayDecision[];
  replayed: ReplayDecision[];
  /** Per-position divergences (missing on either side, or a status/subject differs). */
  mismatches: { index: number; attested?: ReplayDecision; replayed?: ReplayDecision }[];
};

/**
 * Compare the ordered `{ status, subject }` decision sets of the attested chain and a replayed run.
 * Order matters (a dropped, reordered, or status-flipped decision is a mismatch). Crypto-free;
 * computed by the engine, so it needs `@ailu-ai/napi`.
 */
export const verifyReplayDecisions = (
  attested: ReplayDecision[],
  replayed: ReplayDecision[]
): VerifyReplayResult => engineVerifyReplayDecisions(attested, replayed) as VerifyReplayResult;
