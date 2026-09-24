import type { GraphState } from "@ailu-ai/graph-core";
import type { RunEvent } from "@ailu-ai/graph-runtime";

import { readSuspendMeta, SUSPEND_META_KEY, SIGNALS_KEY } from "./durable.js";

/**
 * A structured, machine-readable account of where a run stands (ADR errors-that-teach / AI-DX):
 * what state it's in, why it suspended, what it's waiting for, what failed. An AI agent or a
 * human reads this to decide the next move — resume, deliver a signal, fix an input — without
 * trawling the raw event log. Channel **names** only (never values) so it never leaks payloads.
 */
export type RunExplanation = {
  runId: string;
  status: string;
  currentNode: string;
  /** One-line, human/agent-readable summary of the situation + the next action. */
  summary: string;
  /** Present when suspended: why, on which node, and what unblocks it. */
  suspended?: {
    reason: string;
    node: string;
    awaitingSignal?: string;
    wakeAt?: string;
    /** The concrete next action to resume. */
    nextAction: string;
  };
  /** Present when the run (or a node) failed. */
  failure?: { node?: string; error: string };
  /** The channel names present (not their values). */
  channels: string[];
  /** The last few lifecycle events, type + node only (when an event log is provided). */
  recentEvents?: { type: string; node?: string }[];
};

const RESERVED = new Set([SUSPEND_META_KEY, SIGNALS_KEY]);

/** Channel names a caller actually declared (drop the engine-internal `__*` channels). */
function publicChannels(state: Pick<GraphState, "channels">): string[] {
  return Object.keys(state.channels as Record<string, unknown>)
    .filter((k) => !RESERVED.has(k) && !k.startsWith("__"))
    .sort();
}

/** Find the most recent failure in the event log, if any. */
function findFailure(events: readonly RunEvent[]): { node?: string; error: string } | undefined {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const e = events[i];
    if (e === undefined) continue;
    if (e.type === "node_failed") return { node: String(e.nodeId), error: e.error };
    if (e.type === "run_failed") return { error: e.error };
  }
  return undefined;
}

const nextActionFor = (reason: string, awaitingSignal?: string, wakeAt?: string): string => {
  if (awaitingSignal !== undefined) return `deliver the "${awaitingSignal}" signal with app.signal(runId, "${awaitingSignal}", payload)`;
  if (wakeAt !== undefined) return `the control-plane scheduler resumes at ${wakeAt}; or call app.resume(runId)`;
  if (reason === "human-gate" || reason === "interrupt") return "a human approves, then call app.resume(runId)";
  return "call app.resume(runId)";
};

/**
 * Explain a run from its {@link GraphState} (and, optionally, its lifecycle event log).
 * Pure + read-only — safe to call on any state.
 */
export function explainRun(state: GraphState, events?: readonly RunEvent[]): RunExplanation {
  const runId = String(state.runId);
  const status = String(state.status);
  const currentNode = String(state.currentNodeId);
  const channels = publicChannels(state);
  const recentEvents = events
    ?.slice(-20)
    .map((e) => ({ type: e.type, node: "nodeId" in e ? String(e.nodeId) : undefined }));

  const explanation: RunExplanation = { runId, status, currentNode, summary: "", channels };
  if (recentEvents !== undefined) explanation.recentEvents = recentEvents;

  if (status === "suspended") {
    const meta = readSuspendMeta(state);
    const reason = meta?.reason ?? "interrupt";
    const nextAction = nextActionFor(reason, meta?.awaitingSignal, meta?.wakeAt);
    explanation.suspended = {
      reason,
      node: currentNode,
      ...(meta?.awaitingSignal !== undefined ? { awaitingSignal: meta.awaitingSignal } : {}),
      ...(meta?.wakeAt !== undefined ? { wakeAt: meta.wakeAt } : {}),
      nextAction
    };
    explanation.summary = `Suspended at "${currentNode}" (${reason}). To continue: ${nextAction}.`;
    return explanation;
  }

  if (status === "failed") {
    const failure = events !== undefined ? findFailure(events) : undefined;
    if (failure !== undefined) explanation.failure = failure;
    explanation.summary = failure
      ? `Failed${failure.node ? ` at "${failure.node}"` : ""}: ${failure.error}`
      : `Failed at "${currentNode}".`;
    return explanation;
  }

  explanation.summary =
    status === "completed"
      ? `Completed. Final channels: ${channels.join(", ") || "(none)"}.`
      : `Status "${status}" at "${currentNode}".`;
  return explanation;
}
