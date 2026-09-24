// LLM Council — governed deliberation (ADR 0013 / ADR 0061). A council dispatches a query to N member
// agents (fan-out), has reviewers rank the ANONYMIZED field, aggregates the ranks, and a chair
// synthesizes the final answer. This module holds the two DETERMINISTIC pure steps — anonymize+shuffle
// and aggregate-ranks — no model, no I/O, replay-faithful (the "not an ensemble trick" core of ADR
// 0013) — plus the `council(...)` graph builder that wires them between the agent fan-outs.

import type { EdgeId, GraphDefinition, GraphId, NodeId } from "@ailu-ai/graph-core";

import { toRustAgentConfig, type AgentNodeConfig } from "./agent-node.js";

/** One member's answer to the query. */
export type MemberAnswer = { memberId: string; content: string };

/** An answer stripped of its author, relabeled + shuffled so a reviewer can't favour its own. */
export type AnonymizedAnswer = { label: string; content: string; memberId: string };

/** Deterministic 32-bit hash (FNV-1a) — for a replay-faithful shuffle. */
const hash = (text: string): number => {
  let h = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
};

const LABELS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/**
 * Strip authorship, relabel `A,B,C,…`, and shuffle deterministically by `seed` (so the same run
 * replays identically). Reviewers see only `{ label, content }`; the `memberId` is retained so the
 * control plane can de-anonymize the audit trail after ranking — never shown to a reviewer.
 */
export const anonymizeAndShuffle = (answers: MemberAnswer[], seed: string): AnonymizedAnswer[] => {
  const ordered = [...answers].sort((a, b) => {
    const ha = hash(`${seed}:${a.memberId}`);
    const hb = hash(`${seed}:${b.memberId}`);
    return ha === hb ? a.memberId.localeCompare(b.memberId) : ha - hb;
  });
  return ordered.map((answer, index) => ({
    label: LABELS[index] ?? `M${index}`,
    content: answer.content,
    memberId: answer.memberId
  }));
};

/**
 * Aggregate reviewer rankings into a consensus order (Borda count). Each ranking is an ordered list
 * of labels, best-first; a label at position `p` of `n` scores `n - p`. Unranked labels score 0.
 * Returns the labels best-first; ties break by label asc (deterministic). A ranking's duplicate or
 * unknown labels are ignored.
 */
export const aggregateRanks = (rankings: string[][], labels: string[]): string[] => {
  const valid = new Set(labels);
  const scores = new Map<string, number>(labels.map((l) => [l, 0]));
  for (const ranking of rankings) {
    const seen = new Set<string>();
    const clean = ranking.filter((l) => valid.has(l) && !seen.has(l));
    for (const l of clean) seen.add(l);
    const n = clean.length;
    clean.forEach((label, position) => {
      scores.set(label, (scores.get(label) ?? 0) + (n - position));
    });
  }
  return [...labels].sort((a, b) => {
    const sa = scores.get(a) ?? 0;
    const sb = scores.get(b) ?? 0;
    return sb === sa ? a.localeCompare(b) : sb - sa;
  });
};

/**
 * Parse a reviewer's free-text reply into an ordered list of labels (the labels it names, in order,
 * deduped) — tolerant of prose like "I rank B first, then A, then C". Only whole words exactly equal
 * to a label count (case-sensitive, so the article "a" is not label A; "Answer" is not label A
 * either). Unknown labels are dropped.
 */
export const parseRanking = (text: string, labels: string[]): string[] => {
  const valid = new Set(labels);
  const seen = new Set<string>();
  const out: string[] = [];
  for (const token of text.split(/[^A-Za-z0-9]+/)) {
    if (valid.has(token) && !seen.has(token)) {
      seen.add(token);
      out.push(token);
    }
  }
  return out;
};

/** A council seat (member / reviewer / chair) — an agent-node config minus its output channel. */
export type CouncilSeat = Omit<AgentNodeConfig, "outputChannel">;

export type CouncilOptions = {
  /** Channel holding the query text. Default `"query"`. */
  queryChannel?: string;
  /** The member agents (fixed N). Each answers the query independently, in parallel. */
  members: CouncilSeat[];
  /** Reviewers that rank the anonymized field. Default: one reviewer per member. */
  reviewers?: CouncilSeat[];
  /** The chair that synthesizes the final answer from the aggregated field (writes `answer`). */
  chair: CouncilSeat;
  /** Suspend on a human gate before the chair synthesizes (high-stakes). Default false. */
  humanGate?: boolean;
  /** Seed for the deterministic anonymize-shuffle (replay). Default `"council"`. */
  seed?: string;
};

const memberChannel = (i: number): string => `member_${i}`;
const reviewChannel = (i: number): string => `review_${i}`;

type CouncilNode = GraphDefinition["nodes"][number];

const agentSeatNode = (id: string, seat: CouncilSeat, outputChannel: string): CouncilNode => ({
  id: id as NodeId,
  type: "agent",
  label: id,
  metadata: { agent: toRustAgentConfig(id, { ...seat, outputChannel }) }
});

const componentNode = (
  id: string,
  kind: string,
  params: Record<string, unknown>
): CouncilNode => ({
  id: id as NodeId,
  type: "action",
  label: id,
  metadata: { component: { kind, params } }
});

const edge = (from: string, to: string): GraphDefinition["edges"][number] => ({
  id: `e_${from}_${to}` as EdgeId,
  from: from as NodeId,
  to: to as NodeId,
  type: "default"
});

/**
 * Build a governed LLM Council (ADR 0013 / ADR 0061) as a native **catalog** GraphDefinition: dispatch
 * → members (fan-out) → `councilAnonymize` → reviewers (fan-out, rank the field) → `councilAggregate` →
 * [optional human gate] → chair synthesis. Members/reviewers/chair are agent-carrier nodes (a member
 * never reviews its own answer; every seat audited); anonymize/aggregate are Rust catalog components
 * (their deterministic logic mirrors the exported pure helpers). Every node carries a `component`/
 * `agent` carrier, so the graph runs on the Rust engine via `runCatalogGraph` — no JS handlers. Fixed N
 * (the member list length) via the runtime fan-out. Returns the definition; run it with
 * `runCatalogGraph(council(...))`.
 */
export const council = (options: CouncilOptions): GraphDefinition => {
  const query = options.queryChannel ?? "query";
  const reviewers = options.reviewers ?? options.members;
  const seed = options.seed ?? "council";
  const memberIds = options.members.map((_, i) => memberChannel(i));
  const reviewIds = reviewers.map((_, i) => reviewChannel(i));

  const channels: GraphDefinition["channels"] = {
    [query]: { type: "string", reducer: "replace", default: "" },
    _dispatch: { type: "string", reducer: "replace", default: "" },
    field: { type: "json", reducer: "replace", default: [] },
    aggregate: { type: "json", reducer: "replace", default: [] },
    answer: { type: "agentResult", reducer: "replace" }
  };
  for (const id of [...memberIds, ...reviewIds]) {
    channels[id] = { type: "agentResult", reducer: "replace" };
  }

  const dispatch: CouncilNode = {
    ...componentNode("dispatch", "textCleaner", { from: query, into: "_dispatch" }),
    fanOut: { parallelTo: memberIds as NodeId[], joinAt: "anonymize" as NodeId }
  };
  const anonymize: CouncilNode = {
    ...componentNode("anonymize", "councilAnonymize", { fromChannels: memberIds, into: "field", seed }),
    fanOut: { parallelTo: reviewIds as NodeId[], joinAt: "aggregate" as NodeId }
  };
  const gate: CouncilNode[] =
    options.humanGate === true ? [{ id: "gate" as NodeId, type: "human-gate", label: "gate" }] : [];

  const nodes: CouncilNode[] = [
    dispatch,
    ...options.members.map((seat, i) => agentSeatNode(memberChannel(i), seat, memberChannel(i))),
    anonymize,
    ...reviewers.map((seat, i) => agentSeatNode(reviewChannel(i), seat, reviewChannel(i))),
    componentNode("aggregate", "councilAggregate", {
      reviewsFrom: reviewIds,
      fieldFrom: "field",
      into: "aggregate"
    }),
    ...gate,
    agentSeatNode("chair", options.chair, "answer")
  ];

  const edges = [
    edge("dispatch", memberChannel(0)),
    edge("anonymize", reviewChannel(0)),
    ...(options.humanGate === true
      ? [edge("aggregate", "gate"), edge("gate", "chair")]
      : [edge("aggregate", "chair")])
  ];

  return {
    id: "council" as GraphId,
    version: "0.0.0",
    name: "council",
    channels,
    nodes,
    edges,
    entryNodeId: "dispatch" as NodeId
  };
};
