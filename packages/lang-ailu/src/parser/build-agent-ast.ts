import type { AgentAST } from "../ast/types.js";
import { createLoc } from "./loc.js";

export const buildAgentAST = (raw: unknown, file: string): AgentAST => {
  const input = (raw ?? {}) as Record<string, unknown>;
  return {
    _kind: "agent",
    _loc: createLoc(file),
    id: typeof input.id === "string" ? input.id : "",
    description: typeof input.description === "string" ? input.description : "",
    prompt: typeof input.prompt === "string" ? input.prompt : "",
    tools: Array.isArray(input.tools) ? input.tools.filter((v): v is string => typeof v === "string") : []
  };
};
