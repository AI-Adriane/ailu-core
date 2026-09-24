import type { GraphState, NodeId } from "@ailu-ai/graph-core";
import type { ArtifactRef } from "@ailu-ai/artifact-store";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import type { WorkingMemory } from "./working-memory.js";

export type AgentId = string & { readonly __brand: "AgentId" };

export type Blocker = {
  code: string;
  message: string;
  nodeId?: NodeId;
};

export type AgentResult = {
  artifacts: ArtifactRef[];
  blockers: Blocker[];
  approvalRequests: Array<{ subject: ArtifactRef | { description: string }; reason: string }>;
  confidence: number;
  reasoning: string;
  requiresHumanReview: boolean;
  /**
   * Token usage summed across the run's LLM calls (ADR 0028 phase 7a — observability / cost).
   * Present on results produced by the Rust engine; the control plane maps it to cost and to
   * span/trace attributes. Omitted when no LLM call was made.
   */
  usage?: {
    promptTokens: number;
    completionTokens: number;
    cacheReadTokens?: number;
    cacheWriteTokens?: number;
  };
  /**
   * The validated structured output (ADR 0029 phase 8): the parsed JSON value that conformed to
   * the agent's `structuredOutput` middleware schema. Present only when that middleware ran and
   * produced a valid value; omitted otherwise (no schema requested, or lenient mode never
   * validated). The shape is the caller's schema — typed as `unknown`, validate/narrow at the
   * boundary.
   */
  structuredOutput?: unknown;
};

export type AgentRunFn<TInput = unknown> = (
  input: TInput,
  state: GraphState,
  context: { memory: BaseStore; workingMemory: WorkingMemory; callbacks?: CallbackManager }
) => Promise<AgentResult>;

export type ConsolidatedAgentResult = AgentResult & {
  channelUpdates: Record<string, unknown>;
  conflicts: string[];
};
