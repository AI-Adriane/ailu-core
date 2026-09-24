export type Loc = {
  line: number;
  col: number;
  file: string;
};

export type AstNodeBase<TKind extends string> = {
  _kind: TKind;
  _loc: Loc;
};

export type PromptAST = AstNodeBase<"prompt"> & {
  name: string;
  template: string;
  variables: string[];
};

export type AgentAST = AstNodeBase<"agent"> & {
  id: string;
  description: string;
  prompt: string;
  tools: string[];
};

export type ChainStepAST = AstNodeBase<"chain_step"> & {
  agentId: string;
  input?: Record<string, unknown>;
};

export type ChainAST = AstNodeBase<"chain"> & {
  id: string;
  steps: ChainStepAST[];
};
