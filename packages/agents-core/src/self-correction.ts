import type { Agent } from "./interfaces.js";
import type { AgentResult } from "./types.js";
import type { GraphState } from "@ailu-ai/graph-core";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import type { WorkingMemory } from "./working-memory.js";

type SelfCorrectionOptions = {
  minConfidence?: number;
  maxCorrections?: number;
};

export class SelfCorrectionWrapper<TInput> implements Agent<TInput> {
  public readonly id;
  public readonly name;
  public readonly description;
  private readonly minConfidence: number;
  private readonly maxCorrections: number;

  public constructor(private readonly wrapped: Agent<TInput>, options: SelfCorrectionOptions = {}) {
    this.id = wrapped.id;
    this.name = `${wrapped.name}:self-corrected`;
    this.description = wrapped.description;
    this.minConfidence = options.minConfidence ?? 0.7;
    this.maxCorrections = options.maxCorrections ?? 2;
  }

  public async run(
    input: TInput,
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory; callbacks?: CallbackManager }
  ): Promise<AgentResult> {
    let result = await this.wrapped.run(input, state, context);
    for (let i = 0; i < this.maxCorrections; i += 1) {
      const needsCorrection = result.blockers.length > 0 || result.confidence < this.minConfidence;
      if (!needsCorrection) {
        return result;
      }
      await context.callbacks?.emit({
        type: "onAgentAction",
        runId: String(state.runId),
        nodeId: String(state.currentNodeId),
        timestamp: new Date().toISOString(),
        action: "self-correction",
        payload: { iteration: i + 1, confidence: result.confidence, blockers: result.blockers }
      });
      result = await this.wrapped.run(
        ({
          ...(input as Record<string, unknown>),
          feedback: {
            confidence: result.confidence,
            blockers: result.blockers
          }
        } as unknown) as TInput,
        state,
        context
      );
    }
    return result;
  }
}
