import type { AgentAST, ChainAST, PromptAST } from "../ast/types.js";
import { buildAgentAST } from "../parser/build-agent-ast.js";
import { buildChainAST } from "../parser/build-chain-ast.js";
import { buildPromptAST } from "../parser/build-prompt-ast.js";
import { parseYaml } from "../parser/parse-yaml.js";
import { transformAgent } from "../transformer/transform-agent.js";
import { transformChain } from "../transformer/transform-chain.js";
import { transformPrompt } from "../transformer/transform-prompt.js";
import type { AgentConfig, ChainDefinition, PromptTemplate } from "../transformer/types.js";
import { validateAgentAST } from "../validator/validate-agent-ast.js";
import { validateChainAST } from "../validator/validate-chain-ast.js";
import { validatePromptAST } from "../validator/validate-prompt-ast.js";
import type { Diagnostic } from "../validator/types.js";

type CompileResult = PromptTemplate | AgentConfig | ChainDefinition;

const detectKind = (raw: unknown): "prompt" | "agent" | "chain" => {
  const input = (raw ?? {}) as Record<string, unknown>;
  if (typeof input._kind === "string" && (input._kind === "prompt" || input._kind === "agent" || input._kind === "chain")) {
    return input._kind;
  }
  if ("template" in input) {
    return "prompt";
  }
  if ("steps" in input) {
    return "chain";
  }
  return "agent";
};

export const compileFile = (
  content: string,
  file: string
): { result?: CompileResult; diagnostics: Diagnostic[] } => {
  const raw = parseYaml(content, file);
  const kind = detectKind(raw);
  if (kind === "prompt") {
    const ast: PromptAST = buildPromptAST(raw, file);
    const diagnostics = validatePromptAST(ast);
    if (diagnostics.some((diag) => diag.severity === "error")) {
      return { diagnostics };
    }
    const result = transformPrompt(ast);
    return { result, diagnostics: [...diagnostics, ...result.diagnostics] };
  }
  if (kind === "chain") {
    const ast: ChainAST = buildChainAST(raw, file);
    const diagnostics = validateChainAST(ast);
    if (diagnostics.some((diag) => diag.severity === "error")) {
      return { diagnostics };
    }
    return { result: transformChain(ast), diagnostics };
  }
  const ast: AgentAST = buildAgentAST(raw, file);
  const diagnostics = validateAgentAST(ast);
  if (diagnostics.some((diag) => diag.severity === "error")) {
    return { diagnostics };
  }
  return { result: transformAgent(ast), diagnostics };
};
