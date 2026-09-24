import { load } from "js-yaml";
import type { GraphDefinition } from "@ailu-ai/graph-core";

import { buildGraphAST } from "../parser/build-graph-ast";
import { transformGraph } from "../transformer/transform-graph";
import { validateGraphAST } from "../validator/validate-graph-ast";
import type { Diagnostic } from "../validator/types";

export const compileGraphFile = (
  content: string,
  file: string
): { result?: GraphDefinition; diagnostics: Diagnostic[] } => {
  const raw = load(content);
  const ast = buildGraphAST(raw, file);
  const diagnostics = validateGraphAST(ast);
  if (diagnostics.some((diagnostic) => diagnostic.severity === "error")) {
    return { diagnostics };
  }
  return {
    result: transformGraph(ast),
    diagnostics
  };
};
