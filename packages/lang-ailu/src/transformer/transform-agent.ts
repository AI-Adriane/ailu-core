import type { AgentAST } from "../ast/types.js";
import type { AgentConfig } from "./types.js";

export const transformAgent = (ast: AgentAST): AgentConfig => ({
  id: ast.id,
  description: ast.description,
  prompt: ast.prompt,
  tools: ast.tools
});
