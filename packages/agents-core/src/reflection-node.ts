import type { Command, NodeId } from "@ailu-ai/graph-core";
import type { LLMGateway } from "../../llm-gateway/src/interfaces.js";
import type { NodeHandler } from "../../graph-runtime/src/interfaces.js";

type ReflectionNodeOptions = {
  llm: LLMGateway;
  previousNodeId: NodeId;
  maxReflections?: number;
  /**
   * Accept the draft when the structured critique's `score` is at least this (0..1), or when it
   * reports `ok: true`. Default 0.8. Only applies when the critique is structured JSON; an
   * unstructured reply falls back to the legacy substring heuristic.
   */
  scoreThreshold?: number;
};

/** Structured critique the reflection model is asked to return. */
export type ReflectionCritique = {
  /** Acceptable as-is. */
  ok: boolean;
  /** Overall quality, 0..1. */
  score: number;
  /** Concrete problems to fix on the next revision. */
  issues: string[];
};

const REFLECTION_COUNT_KEY = "__reflectionCount";
/** Channel carrying the critique's concrete issues back to the draft node on a revision. */
export const REFLECTION_ISSUES_KEY = "__reflectionIssues";
const DEFAULT_SCORE_THRESHOLD = 0.8;

const CRITIQUE_INSTRUCTION =
  "Critique the output below. Respond ONLY with JSON of the form " +
  '{"ok": boolean, "score": number between 0 and 1, "issues": string[]}. ' +
  "`ok` is true when the output is acceptable as-is; `score` is overall quality; " +
  "`issues` lists concrete, actionable problems to fix.";

/**
 * Tolerantly extract a {@link ReflectionCritique} from a model reply — the JSON may be wrapped in
 * prose or a markdown fence. Returns `null` when no structured critique is present (caller then
 * falls back to the substring heuristic). Out-of-range scores are clamped to 0..1.
 */
export const parseReflectionCritique = (raw: string): ReflectionCritique | null => {
  const match = raw.match(/\{[\s\S]*\}/);
  if (match === null) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(match[0]);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) {
    return null;
  }
  const obj = parsed as Record<string, unknown>;
  const ok = typeof obj.ok === "boolean" ? obj.ok : undefined;
  const score =
    typeof obj.score === "number" && Number.isFinite(obj.score) ? Math.max(0, Math.min(1, obj.score)) : undefined;
  if (ok === undefined && score === undefined) {
    return null; // a JSON object that isn't a critique
  }
  const issues = Array.isArray(obj.issues) ? obj.issues.filter((i): i is string => typeof i === "string") : [];
  return { ok: ok ?? false, score: score ?? (ok ? 1 : 0), issues };
};

/**
 * Decide whether the draft needs another revision and surface the issues to fix. A structured
 * critique revises unless `ok` or `score >= scoreThreshold`; an unstructured reply falls back to
 * the legacy `"problem"`/`"retry"` substring heuristic (no issues).
 */
export const critiqueRequestsRevision = (
  raw: string,
  scoreThreshold: number = DEFAULT_SCORE_THRESHOLD
): { revise: boolean; issues: string[] } => {
  const structured = parseReflectionCritique(raw);
  if (structured !== null) {
    const accept = structured.ok || structured.score >= scoreThreshold;
    return { revise: !accept, issues: structured.issues };
  }
  const lower = raw.toLowerCase();
  return { revise: lower.includes("problem") || lower.includes("retry"), issues: [] };
};

export const createReflectionNode = (options: ReflectionNodeOptions): NodeHandler => {
  const maxReflections = options.maxReflections ?? 2;
  const scoreThreshold = options.scoreThreshold ?? DEFAULT_SCORE_THRESHOLD;
  return async (input) => {
    const channels = input as Record<string, unknown>;
    const count = typeof channels[REFLECTION_COUNT_KEY] === "number" ? (channels[REFLECTION_COUNT_KEY] as number) : 0;
    const completion = await options.llm.complete({
      provider: "openai",
      model: "reflection-node",
      messages: [
        {
          role: "user",
          content: `${CRITIQUE_INSTRUCTION}\n\nOutput to critique: ${JSON.stringify(input)}`
        }
      ]
    });
    const { revise, issues } = critiqueRequestsRevision(completion.content, scoreThreshold);
    if (count < maxReflections && revise) {
      const cmd: Command = {
        goto: options.previousNodeId,
        update: { [REFLECTION_COUNT_KEY]: count + 1, [REFLECTION_ISSUES_KEY]: issues } as never
      };
      return cmd;
    }
    return {
      ...channels,
      confidence: Math.min(1, (typeof channels.confidence === "number" ? (channels.confidence as number) : 0.5) + 0.1),
      [REFLECTION_COUNT_KEY]: count
    };
  };
};
