import type { GraphState } from "@ailu/graph-core";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import type { WorkingMemory } from "./working-memory.js";

import type { AgentId, AgentResult } from "./types.js";

export interface Agent<TInput = unknown> {
  id: AgentId;
  name: string;
  description: string;
  run(
    input: TInput,
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory; callbacks?: CallbackManager }
  ): Promise<AgentResult>;
}

export interface AgentRegistry {
  register(agent: Agent): void;
  resolve(id: AgentId): Agent | undefined;
  list(): Agent[];
}
