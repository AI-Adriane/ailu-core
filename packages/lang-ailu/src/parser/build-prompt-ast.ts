import type { PromptAST } from "../ast/types.js";
import { createLoc } from "./loc.js";

export const buildPromptAST = (raw: unknown, file: string): PromptAST => {
  const input = (raw ?? {}) as Record<string, unknown>;
  return {
    _kind: "prompt",
    _loc: createLoc(file),
    name: typeof input.name === "string" ? input.name : "",
    template: typeof input.template === "string" ? input.template : "",
    variables: Array.isArray(input.variables) ? input.variables.filter((v): v is string => typeof v === "string") : []
  };
};
