import type { AgentAST } from "../ast/types.js";
import type { Diagnostic } from "./types.js";

export const validateAgentAST = (ast: AgentAST): Diagnostic[] => {
  const diagnostics: Diagnostic[] = [];
  if (ast.id.trim().length === 0) {
    diagnostics.push({
      code: "AGENT_ID_REQUIRED",
      message: "Agent id is required.",
      loc: ast._loc,
      severity: "error"
    });
  }
  if (ast.prompt.trim().length === 0) {
    diagnostics.push({
      code: "AGENT_PROMPT_REQUIRED",
      message: "Agent prompt reference is required.",
      loc: ast._loc,
      severity: "error"
    });
  }
  return diagnostics;
};
