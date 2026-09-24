import type { PromptAST } from "../ast/types.js";
import type { Diagnostic } from "./types.js";

export const validatePromptAST = (ast: PromptAST): Diagnostic[] => {
  const diagnostics: Diagnostic[] = [];
  if (ast.name.trim().length === 0) {
    diagnostics.push({
      code: "PROMPT_NAME_REQUIRED",
      message: "Prompt name is required.",
      loc: ast._loc,
      severity: "error"
    });
  }
  if (ast.template.trim().length === 0) {
    diagnostics.push({
      code: "PROMPT_TEMPLATE_REQUIRED",
      message: "Prompt template is required.",
      loc: ast._loc,
      severity: "error"
    });
  }
  return diagnostics;
};
