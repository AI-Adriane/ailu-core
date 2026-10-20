//! Replay-as-evidence (ADR 0038): the decisions a signed chain attests — the attested side of
//! {@link verifyReplayDecisions}, next to {@link replayedDecisions}.
//!
//! A request that needs several signatures (a quorum) is signed once per signer, so the chain
//! holds several links for one request: the signatures given before the last, then the one that
//! resolved it. A replay re-derives the request once. The decision is the request's last link —
//! the resolution — so one decision per request, at the place its resolution takes in the chain.
//!
//! Two links are the same request only when every signed field that names what was decided is
//! the same: `runId`, `approvalId`, `subject`, `callKey` and `grant`. Matching on `approvalId`
//! alone would let a link that signs another decision under the same id (a tool call, another
//! gate) disappear behind the last one. The signer-specific fields (`resolvedBy`, `decidedAt`)
//! and `status` are not part of the identity.

import type { AttestationRecord } from "@ailu-ai/approval-engine";

import type { ReplayDecision } from "./verify-replay.js";

/** The signed fields of a link that say what it decided. */
type DecisionLink = Pick<
  AttestationRecord,
  "runId" | "approvalId" | "subject" | "callKey" | "grant"
>;

/** What makes two links the same request: every signed field that names the decision. */
const identityOf = (link: DecisionLink): string =>
  JSON.stringify([
    link.runId,
    link.approvalId,
    link.subject,
    link.callKey ?? null,
    link.grant ?? null
  ]);

/**
 * The decisions a chain attests, in chain order: one per request, its last link, with the call it
 * signed when it signed one (ADR 0051 D3). Links of the same `approvalId` that sign another
 * subject or call stay separate decisions. Status is left blank, as on the replayed side: the
 * chain itself proves who decided what. Check the chain with `verifyChain` first — this reads the
 * records, it does not verify them.
 */
export const attestedDecisions = (records: readonly DecisionLink[]): ReplayDecision[] => {
  const identities = records.map(identityOf);
  return records
    .filter((_, index) => !identities.slice(index + 1).includes(identities[index] ?? ""))
    .map((record) => ({
      status: "",
      subject: record.subject,
      ...(record.callKey !== undefined ? { callKey: record.callKey } : {})
    }));
};
