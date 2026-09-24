import type { GraphState } from "@ailu-ai/graph-core";

import type { Agent } from "./interfaces.js";
import type { AgentResult } from "./types.js";

export type CoordinationTask<TInput> = {
  agent: Agent<TInput>;
  input: TInput;
  proposedUpdate?: Record<string, unknown>;
};

export type CoordinationResult = AgentResult & {
  channelUpdates: Record<string, unknown>;
  conflicts: string[];
};

export class AgentCoordinator {
  public async runParallel<TInput>(
    tasks: CoordinationTask<TInput>[],
    state: GraphState,
    context: Parameters<Agent<TInput>["run"]>[2]
  ): Promise<CoordinationResult> {
    const results = await Promise.all(tasks.map((task) => task.agent.run(task.input, state, context)));
    const channelUpdates: Record<string, unknown> = {};
    const conflicts: string[] = [];

    for (const task of tasks) {
      const updates = task.proposedUpdate ?? {};
      for (const [key, value] of Object.entries(updates)) {
        if (key in channelUpdates && JSON.stringify(channelUpdates[key]) !== JSON.stringify(value)) {
          conflicts.push(key);
          continue;
        }
        channelUpdates[key] = value;
      }
    }

    const confidence = results.length === 0 ? 0 : results.reduce((sum, result) => sum + result.confidence, 0) / results.length;

    return {
      artifacts: results.flatMap((result) => result.artifacts),
      blockers: results.flatMap((result) => result.blockers),
      approvalRequests: results.flatMap((result) => result.approvalRequests),
      confidence,
      reasoning: results.map((result) => result.reasoning).join("\n---\n"),
      requiresHumanReview: results.some((result) => result.requiresHumanReview),
      channelUpdates,
      conflicts
    };
  }
}
