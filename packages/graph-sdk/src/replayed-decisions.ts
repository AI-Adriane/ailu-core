//! Replay-as-evidence (ADR 0038): the decisions a replay re-derives — the replayed side of
//! {@link verifyReplayDecisions}, the same for every host (the control plane, `@ailu-ai/verify`).
//!
//! A replay files nothing. Its `pendingApprovals` name what an agent asked for (`tool:<name>`),
//! but never the human gate it stopped at: that request (`gate:<node id>`) is filed by the run
//! that suspends there. Read alone, a replay of any run with a human gate reproduces one decision
//! fewer than its chain attests. The decisions a replay re-derives are, by construction, the
//! requests a run files where the replay stopped — so they are read from the engine's own filing
//! plan (`filing_plan`, ADR 0045 D3.1): an agent's tool requests, then the human gate it stops at,
//! then those of a direct child run suspended inside a subgraph node. One rule, the engine's.

import type { GraphDefinition, GraphState } from "@ailu-ai/graph-core";

import { APPROVAL_IDS_CHANNEL } from "./agent-node.js";
import { engineApprovalPlan } from "./rust-engine.js";
import type { ReplayDecision } from "./verify-replay.js";

/** What {@link replayedDecisions} reads of a replay: where it stopped, and its state. */
export type ReplayStop = {
  /** The replay's status; only a `"suspended"` replay re-derived a decision. */
  status: string;
  state: GraphState;
};

/**
 * The decisions a replay re-derives, in the order a run files them: an agent's tool requests
 * (each with the `callKey` of its call, ADR 0051 D3), then the human gate the replay stopped at.
 * Status is left blank: whether a request was approved is the human's decision, which the
 * attestation chain proves, not something a re-execution reproduces. Nothing for a replay that
 * did not suspend.
 *
 * Pass the outcome of {@link replayCatalogGraph} and the graph (and subgraphs) it replayed; compare
 * the result with the signed records through {@link verifyReplayDecisions}. Needs the native
 * engine (`@ailu-ai/napi`), as the replay does.
 */
export const replayedDecisions = (
  outcome: ReplayStop,
  graph: GraphDefinition,
  subgraphs: GraphDefinition[] = []
): ReplayDecision[] => {
  if (outcome.status !== "suspended") return [];
  // A replay stashed no request id: ask what a run stopping here files, whatever ids the state
  // carried from its recording.
  const state: GraphState = {
    ...outcome.state,
    channels: { ...(outcome.state.channels as Record<string, unknown>), [APPROVAL_IDS_CHANNEL]: [] }
  };
  return engineApprovalPlan({ graph, subgraphs, state }).requests.map(({ subject }) => ({
    status: "",
    subject: subject.description,
    ...(subject.callKey !== undefined ? { callKey: subject.callKey } : {})
  }));
};
