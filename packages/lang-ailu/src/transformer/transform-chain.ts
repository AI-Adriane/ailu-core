import type { ChainAST } from "../ast/types.js";
import type { ChainDefinition } from "./types.js";

export const transformChain = (ast: ChainAST): ChainDefinition => ({
  id: ast.id,
  steps: ast.steps.map((step) => ({
    agentId: step.agentId,
    input: step.input
  }))
});
