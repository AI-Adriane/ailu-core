//! Replay-as-evidence (ADR 0038): the decisions a signed chain attests — the attested side of
//! {@link verifyReplayDecisions}, next to {@link replayedDecisions}.
//!
//! A request that needs several signatures (a quorum) is signed once per signer, so the chain
//! holds several links for one request: the signatures given before the last, then the one that
//! resolved it. A replay re-derives the request once. The decision is the request's last link —
//! the resolution — so one decision per request, at the place its resolution takes in the chain.
//!
//! The links of one `approvalId` (of one run) are one quorum only when they all say the same thing
//! and come from distinct signers: the same `subject` and the same `callKey`, a `grant` that is the
//! resolution's (an earlier signature may leave it out, none may name another), the same `status`,
//! and no signer twice. When they do not, nothing is collapsed: every link stays its own decision,
//! so a link that signs another call, another gate, another status or a repeated signature
//! cannot disappear behind the last one, and the replay does not reproduce the chain.

import type { AttestationRecord } from "@ailu-ai/approval-engine";

import type { ReplayDecision } from "./verify-replay.js";

/** The signed fields of a link that say what it decided, and who signed it. */
type DecisionLink = Pick<
  AttestationRecord,
  "runId" | "approvalId" | "subject" | "callKey" | "grant"
> &
  Partial<Pick<AttestationRecord, "resolvedBy" | "status">>;

/** Which request a link belongs to: its run and its approval id. */
const requestOf = (link: DecisionLink): string => JSON.stringify([link.runId, link.approvalId]);

/**
 * Whether the links of one request, in chain order, are one quorum: each says what the last one
 * (the resolution) says, and each is signed by a signer not seen before. The signer and the
 * status are checked when the link carries them.
 */
const isQuorum = (links: readonly DecisionLink[]): boolean => {
  const last = links.at(-1);
  if (last === undefined) return false;
  const signers = new Set<string>();
  return links.every((link) => {
    if (link.subject !== last.subject) return false;
    if ((link.callKey ?? null) !== (last.callKey ?? null)) return false;
    if (link.grant !== undefined && link.grant !== last.grant) return false;
    if (link.status !== undefined && last.status !== undefined && link.status !== last.status) {
      return false;
    }
    if (link.resolvedBy !== undefined) {
      if (signers.has(link.resolvedBy)) return false;
      signers.add(link.resolvedBy);
    }
    return true;
  });
};

/**
 * The decisions a chain attests, in chain order: one per request, its last link, with the call it
 * signed when it signed one (ADR 0051 D3). The links of a request that is not a clean quorum (see
 * the top of this file) stay separate decisions. Status is left blank, as on the replayed side:
 * the chain itself proves who decided what. Check the chain with `verifyChain` first — this reads
 * the records, it does not verify them.
 */
export const attestedDecisions = (records: readonly DecisionLink[]): ReplayDecision[] => {
  const byRequest = new Map<string, number[]>();
  records.forEach((record, index) => {
    const key = requestOf(record);
    byRequest.set(key, [...(byRequest.get(key) ?? []), index]);
  });
  const kept = new Set<number>();
  for (const indexes of byRequest.values()) {
    const links = indexes.map((index) => records[index] as DecisionLink);
    if (isQuorum(links)) kept.add(indexes.at(-1) as number);
    else indexes.forEach((index) => kept.add(index));
  }
  return records
    .filter((_, index) => kept.has(index))
    .map((record) => ({
      status: "",
      subject: record.subject,
      ...(record.callKey !== undefined ? { callKey: record.callKey } : {})
    }));
};
