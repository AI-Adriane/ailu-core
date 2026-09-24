import type { GraphAst } from "../ast/types";
import { parseVersionedRef } from "./ref";

const loc = (file: string) => ({ line: 1, col: 1, file });

export const buildGraphAST = (raw: unknown, file: string): GraphAst => {
  const input = (raw ?? {}) as Record<string, unknown>;
  const nodesRaw = Array.isArray(input.nodes) ? input.nodes : [];
  const edgesRaw = Array.isArray(input.edges) ? input.edges : [];
  const channelsRaw =
    input.channels !== null && typeof input.channels === "object" && !Array.isArray(input.channels)
      ? (input.channels as Record<string, unknown>)
      : {};

  const normalizeReducer = (value: unknown): "replace" | "append" | "merge" => {
    if (value === "append" || value === "merge" || value === "replace") {
      return value;
    }
    return "replace";
  };
  const normalizeNodeType = (
    value: unknown
  ): "action" | "agent" | "tool" | "human-gate" | "subgraph" => {
    if (value === "agent" || value === "tool" || value === "human-gate" || value === "subgraph" || value === "action") {
      return value;
    }
    return "action";
  };
  const normalizeEdgeType = (value: unknown): "default" | "conditional" =>
    value === "conditional" ? "conditional" : "default";

  const channels = Object.entries(channelsRaw).map(([name, definition]) => {
    const def = (definition ?? {}) as Record<string, unknown>;
    return {
      name,
      type: typeof def.type === "string" ? def.type : "unknown",
      reducer: normalizeReducer(def.reducer),
      default: def.default,
      _loc: loc(file)
    };
  });

  const nodes = nodesRaw.map((nodeRaw) => {
    const node = (nodeRaw ?? {}) as Record<string, unknown>;
    const subgraph = typeof node.graph === "string" ? parseVersionedRef(node.graph) : undefined;
    return {
      id: typeof node.id === "string" ? node.id : "",
      type: normalizeNodeType(node.type),
      label: typeof node.label === "string" ? node.label : "",
      subgraph,
      _loc: loc(file)
    };
  });

  const edges = edgesRaw.map((edgeRaw) => {
    const edge = (edgeRaw ?? {}) as Record<string, unknown>;
    const condition = typeof edge.condition === "string" ? { value: edge.condition, _loc: loc(file) } : undefined;
    return {
      id: typeof edge.id === "string" ? edge.id : "",
      from: typeof edge.from === "string" ? edge.from : "",
      to: typeof edge.to === "string" ? edge.to : "",
      type: normalizeEdgeType(edge.type),
      condition,
      _loc: loc(file)
    };
  });

  return {
    id: typeof input.id === "string" ? input.id : "",
    version: typeof input.version === "string" ? input.version : "",
    name: typeof input.name === "string" ? input.name : "",
    recursionLimit: typeof input.recursionLimit === "number" ? input.recursionLimit : undefined,
    entryNodeId: typeof input.entryNodeId === "string" ? input.entryNodeId : "",
    channels,
    nodes,
    edges,
    _loc: loc(file)
  };
};
