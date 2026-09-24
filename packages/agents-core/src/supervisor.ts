import type { Command, GraphState, NodeId } from "@ailu/graph-core";

import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import type { LLMGateway } from "../../llm-gateway/src/interfaces.js";
import type { Agent } from "./interfaces.js";
import type { AgentId, AgentResult } from "./types.js";
import type { WorkingMemory } from "./working-memory.js";

export type SupervisorConfig = {
  agents: AgentId[];
  maxRounds: number;
};

type SupervisorOptions = {
  id: AgentId;
  name: string;
  description: string;
  llm: LLMGateway;
  config: SupervisorConfig;
  agentNodeMap: Record<string, NodeId>;
  agentDescriptions: Record<string, string>;
};

export class SupervisorAgent implements Agent<{ objective: string }> {
  public readonly id: AgentId;
  public readonly name: string;
  public readonly description: string;
  private readonly llm: LLMGateway;
  private readonly config: SupervisorConfig;
  private readonly agentNodeMap: Record<string, NodeId>;
  private readonly agentDescriptions: Record<string, string>;

  public constructor(options: SupervisorOptions) {
    this.id = options.id;
    this.name = options.name;
    this.description = options.description;
    this.llm = options.llm;
    this.config = options.config;
    this.agentNodeMap = options.agentNodeMap;
    this.agentDescriptions = options.agentDescriptions;
  }

  public async nextCommand(input: { objective: string }, state: GraphState): Promise<Command | "FINISH"> {
    const rounds = ((state.channels as Record<string, unknown>).__supervisorRounds as number | undefined) ?? 0;
    if (rounds >= this.config.maxRounds) {
      return "FINISH";
    }
    const candidates = this.config.agents
      .map((id) => `- ${String(id)}: ${this.agentDescriptions[String(id)] ?? "No description"}`)
      .join("\n");
    const completion = await this.llm.complete({
      provider: "openai",
      model: "supervisor-router",
      messages: [
        {
          role: "user",
          content: `Objective: ${input.objective}\nAgents:\n${candidates}\nReply with AGENT:<id> or FINISH`
        }
      ]
    });
    const response = completion.content.trim();
    if (response.startsWith("FINISH")) {
      return "FINISH";
    }
    const selected = response.replace("AGENT:", "").trim();
    const selectedId = this.config.agents.find((agentId) => String(agentId) === selected);
    if (selectedId === undefined) {
      return "FINISH";
    }
    const target = this.agentNodeMap[String(selectedId)];
    if (target === undefined) {
      return "FINISH";
    }
    return {
      goto: target,
      update: {
        input: { objective: input.objective },
        __supervisorRounds: rounds + 1
      } as never
    };
  }

  public async run(
    input: { objective: string },
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory; callbacks?: CallbackManager }
  ): Promise<AgentResult> {
    void context;
    const decision = await this.nextCommand(input, state);
    return {
      artifacts: [],
      blockers: [],
      approvalRequests: [],
      confidence: decision === "FINISH" ? 1 : 0.8,
      reasoning: decision === "FINISH" ? "FINISH" : JSON.stringify(decision),
      requiresHumanReview: false
    };
  }
}
