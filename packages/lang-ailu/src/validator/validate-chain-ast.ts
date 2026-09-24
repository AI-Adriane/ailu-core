import type { ChainAST } from "../ast/types.js";
import type { Diagnostic } from "./types.js";

export const validateChainAST = (ast: ChainAST): Diagnostic[] => {
  const diagnostics: Diagnostic[] = [];
  if (ast.id.trim().length === 0) {
    diagnostics.push({
      code: "CHAIN_ID_REQUIRED",
      message: "Chain id is required.",
      loc: ast._loc,
      severity: "error"
    });
  }
  if (ast.steps.length === 0) {
    diagnostics.push({
      code: "CHAIN_STEPS_REQUIRED",
      message: "Chain must contain at least one step.",
      loc: ast._loc,
      severity: "error"
    });
  }
  for (const step of ast.steps) {
    if (step.agentId.trim().length === 0) {
      diagnostics.push({
        code: "CHAIN_STEP_AGENT_REQUIRED",
        message: "Each chain step must reference an agentId.",
        loc: step._loc,
        severity: "error"
      });
    }
  }
  return diagnostics;
};
