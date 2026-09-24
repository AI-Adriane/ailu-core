export type Loc = {
  line: number;
  col: number;
  file: string;
};

export type VersionedRef = {
  id: string;
  version: string;
};

export type ConditionAst = {
  value: string;
  _loc: Loc;
};

export type ChannelAst = {
  name: string;
  type: string;
  reducer: "replace" | "append" | "merge";
  default?: unknown;
  _loc: Loc;
};

export type NodeAst = {
  id: string;
  type: "action" | "agent" | "tool" | "human-gate" | "subgraph";
  label: string;
  subgraph?: VersionedRef;
  _loc: Loc;
};

export type EdgeAst = {
  id: string;
  from: string;
  to: string;
  type: "default" | "conditional";
  condition?: ConditionAst;
  _loc: Loc;
};

export type GraphAst = {
  id: string;
  version: string;
  name: string;
  recursionLimit?: number;
  entryNodeId: string;
  channels: ChannelAst[];
  nodes: NodeAst[];
  edges: EdgeAst[];
  _loc: Loc;
};
