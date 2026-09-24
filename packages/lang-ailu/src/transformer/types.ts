import type { Diagnostic } from "../validator/types.js";

export type PromptTemplate = {
  name: string;
  template: string;
  diagnostics: Diagnostic[];
  render: (variables: Record<string, unknown>) => { content: string; diagnostics: Diagnostic[] };
};

export type AgentConfig = {
  id: string;
  description: string;
  prompt: string;
  tools: string[];
};

export type ChainDefinition = {
  id: string;
  steps: Array<{
    agentId: string;
    input?: Record<string, unknown>;
  }>;
};
