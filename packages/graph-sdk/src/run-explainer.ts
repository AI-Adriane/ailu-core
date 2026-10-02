import type { GraphState } from "@ailu-ai/graph-core";
import type { RunEvent } from "@ailu-ai/graph-runtime";

import { engineExplainRun } from "./rust-engine.js";

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

/**
 * Explain a run from its {@link GraphState} (and, optionally, its lifecycle event log).
 * Pure + read-only — safe to call on any state. The account is the engine's (`explain_run`, ADR
 * 0045 D3.4), the same for every SDK, so it needs `@ailu-ai/napi`.
 */
export function explainRun(state: GraphState, events?: readonly RunEvent[]): RunExplanation {
  return engineExplainRun(state, events) as RunExplanation;
}
