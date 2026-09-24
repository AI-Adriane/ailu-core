import type { ChainAST, ChainStepAST } from "../ast/types.js";
import { createLoc } from "./loc.js";

export const buildChainAST = (raw: unknown, file: string): ChainAST => {
  const input = (raw ?? {}) as Record<string, unknown>;
  const stepsRaw = Array.isArray(input.steps) ? input.steps : [];
  const steps: ChainStepAST[] = stepsRaw.map((stepRaw) => {
    const step = (stepRaw ?? {}) as Record<string, unknown>;
    return {
      _kind: "chain_step",
      _loc: createLoc(file),
      agentId: typeof step.agentId === "string" ? step.agentId : "",
      input:
        step.input !== null && typeof step.input === "object" && !Array.isArray(step.input)
          ? (step.input as Record<string, unknown>)
          : undefined
    };
  });

  return {
    _kind: "chain",
    _loc: createLoc(file),
    id: typeof input.id === "string" ? input.id : "",
    steps
  };
};
