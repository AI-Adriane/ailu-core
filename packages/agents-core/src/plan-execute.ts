import type { Agent } from "./interfaces.js";
import type { AgentId, AgentResult } from "./types.js";
import type { LLMGateway } from "../../llm-gateway/src/interfaces.js";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { GraphState } from "@ailu-ai/graph-core";
import type { WorkingMemory } from "./working-memory.js";

type PlanStep = { id: string; text: string };

export class PlannerAgent implements Agent<{ objective: string }> {
  public readonly id: AgentId;
  public readonly name: string;
  public readonly description: string;

  public constructor(private readonly llm: LLMGateway, id: AgentId = "planner-agent" as AgentId) {
    this.id = id;
    this.name = "PlannerAgent";
    this.description = "Generates a plan from objective.";
  }

  public async run(
    input: { objective: string },
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory }
  ): Promise<AgentResult> {
    const completion = await this.llm.complete({
      provider: "openai",
      model: "planner",
      messages: [{ role: "user", content: `Plan objective: ${input.objective}` }]
    });
    const steps = completion.content
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line.length > 0)
      .map((line, index) => ({ id: `step-${index + 1}`, text: line }));
    const finalSteps: PlanStep[] = steps.length > 0 ? steps : [{ id: "step-1", text: input.objective }];
    await context.memory.put(["plan", String(state.runId)], "steps", finalSteps);
    return {
      artifacts: [],
      blockers: [],
      approvalRequests: [],
      confidence: 0.8,
      reasoning: JSON.stringify(finalSteps),
      requiresHumanReview: false
    };
  }
}

export class ExecutorAgent implements Agent<{ steps: PlanStep[] }> {
  public readonly id: AgentId;
  public readonly name = "ExecutorAgent";
  public readonly description = "Executes plan steps sequentially.";

  public constructor(
    private readonly executeStep: (step: PlanStep) => Promise<string>,
    id: AgentId = "executor-agent" as AgentId
  ) {
    this.id = id;
  }

  public async run(
    input: { steps: PlanStep[] },
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory }
  ): Promise<AgentResult> {
    const logs: string[] = [];
    for (const step of input.steps) {
      logs.push(await this.executeStep(step));
    }
    await context.memory.put(["plan", String(state.runId)], "execution", logs);
    return {
      artifacts: [],
      blockers: [],
      approvalRequests: [],
      confidence: 0.85,
      reasoning: logs.join("\n"),
      requiresHumanReview: false
    };
  }
}
