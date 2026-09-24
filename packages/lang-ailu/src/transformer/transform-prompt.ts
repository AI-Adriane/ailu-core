import type { PromptAST } from "../ast/types.js";
import type { PromptTemplate } from "./types.js";
import { detectUnresolvedTemplateVariables, renderTemplate } from "./template-engine.js";

export const transformPrompt = (ast: PromptAST): PromptTemplate => ({
  name: ast.name,
  template: ast.template,
  diagnostics: detectUnresolvedTemplateVariables(ast.template, ast.variables, ast._loc),
  render: (variables) => renderTemplate(ast.template, variables, ast._loc)
});
