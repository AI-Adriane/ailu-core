import type { GraphAst } from "../ast/types";
import { isValidSemver } from "../parser/ref";
import type { Diagnostic } from "./types";

const hasCycle = (ast: GraphAst): boolean => {
  const adj = new Map<string, string[]>();
  for (const node of ast.nodes) {
    adj.set(node.id, []);
  }
  for (const edge of ast.edges) {
    const list = adj.get(edge.from) ?? [];
    list.push(edge.to);
    adj.set(edge.from, list);
  }
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const dfs = (id: string): boolean => {
    if (visiting.has(id)) {
      return true;
    }
    if (visited.has(id)) {
      return false;
    }
    visiting.add(id);
    for (const next of adj.get(id) ?? []) {
      if (dfs(next)) {
        return true;
      }
    }
    visiting.delete(id);
    visited.add(id);
    return false;
  };
  for (const node of ast.nodes) {
    if (dfs(node.id)) {
      return true;
    }
  }
  return false;
};

export const validateGraphAST = (ast: GraphAst): Diagnostic[] => {
  const diagnostics: Diagnostic[] = [];
  const nodeIds = new Set(ast.nodes.map((node) => node.id));

  if (!nodeIds.has(ast.entryNodeId)) {
    diagnostics.push({
      code: "ENTRY_NODE_NOT_FOUND",
      message: `Entry node '${ast.entryNodeId}' does not exist.`,
      loc: ast._loc,
      severity: "error"
    });
  }

  for (const edge of ast.edges) {
    if (!nodeIds.has(edge.from) || !nodeIds.has(edge.to)) {
      diagnostics.push({
        code: "EDGE_NODE_NOT_FOUND",
        message: `Edge '${edge.id}' references unknown nodes.`,
        loc: edge._loc,
        severity: "error"
      });
    }
    if (edge.condition !== undefined && edge.condition.value.trim().length === 0) {
      diagnostics.push({
        code: "EDGE_CONDITION_EMPTY",
        message: `Edge '${edge.id}' has an empty condition.`,
        loc: edge.condition._loc,
        severity: "error"
      });
    }
  }

  for (const node of ast.nodes) {
    if (node.type === "subgraph") {
      if (node.subgraph === undefined) {
        diagnostics.push({
          code: "SUBGRAPH_REF_REQUIRED",
          message: `Subgraph node '${node.id}' requires a graph reference.`,
          loc: node._loc,
          severity: "error"
        });
      } else if (!isValidSemver(node.subgraph.version)) {
        diagnostics.push({
          code: "SUBGRAPH_REF_VERSION_INVALID",
          message: `Subgraph ref version '${node.subgraph.version}' is invalid semver.`,
          loc: node._loc,
          severity: "error"
        });
      }
    }
  }

  for (const channel of ast.channels) {
    if (!(channel.reducer === "replace" || channel.reducer === "append" || channel.reducer === "merge")) {
      diagnostics.push({
        code: "CHANNEL_REDUCER_INVALID",
        message: `Channel '${channel.name}' has invalid reducer '${String(channel.reducer)}'.`,
        loc: channel._loc,
        severity: "error"
      });
    }
  }

  if (hasCycle(ast) && ast.recursionLimit === undefined) {
    diagnostics.push({
      code: "CYCLE_WITHOUT_RECURSION_LIMIT",
      message: "Graph contains cycles but recursionLimit is missing.",
      loc: ast._loc,
      severity: "warning"
    });
  }

  return diagnostics;
};
