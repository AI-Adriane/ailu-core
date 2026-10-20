//! Replay-as-evidence (ADR 0038): the decisions a signed chain attests — the attested side of
//! {@link verifyReplayDecisions}, next to {@link replayedDecisions}.
//!
//! A request that needs several signatures (a quorum) is signed once per signer, so the chain
//! holds several links for one request: the signatures given before the last, then the one that
//! resolved it. A replay re-derives the request once. The decision is the request's last link —
//! the resolution — so one decision per request, at the place its resolution takes in the chain.
//! Read only from the fields each link signs (`approvalId`, `subject`, `callKey`).

import type { AttestationRecord } from "@ailu-ai/approval-engine";

import type { ReplayDecision } from "./verify-replay.js";

/**
 * The decisions a chain attests, in chain order: one per request (`approvalId`), its last link,
 * with the call it signed when it signed one (ADR 0051 D3). Status is left blank, as on the
 * replayed side: the chain itself proves who decided what. Check the chain with `verifyChain`
 * first — this reads the records, it does not verify them.
 */
export const attestedDecisions = (
  records: readonly Pick<AttestationRecord, "approvalId" | "subject" | "callKey">[]
): ReplayDecision[] =>
  records
    .filter(
      (record, index) =>
        !records.slice(index + 1).some((later) => later.approvalId === record.approvalId)
    )
    .map((record) => ({
      status: "",
      subject: record.subject,
      ...(record.callKey !== undefined ? { callKey: record.callKey } : {})
    }));
