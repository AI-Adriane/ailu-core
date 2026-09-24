import type { GraphDefinition } from "@ailu/graph-core";

import type { GraphAst } from "../ast/types";

export const transformGraph = (ast: GraphAst): GraphDefinition => ({
  id: ast.id as GraphDefinition["id"],
  version: ast.version,
  name: ast.name,
  recursionLimit: ast.recursionLimit,
  entryNodeId: ast.entryNodeId as GraphDefinition["entryNodeId"],
  channels: Object.fromEntries(
    ast.channels.map((channel) => [
      channel.name,
      {
        type: channel.type,
        reducer: channel.reducer,
        default: channel.default
      }
    ])
  ),
  nodes: ast.nodes.map((node) => ({
    id: node.id as GraphDefinition["nodes"][number]["id"],
    type: node.type,
    label: node.label,
    subgraphId: node.subgraph?.id as GraphDefinition["id"] | undefined
  })),
  edges: ast.edges.map((edge) => ({
    id: edge.id as GraphDefinition["edges"][number]["id"],
    from: edge.from as GraphDefinition["nodes"][number]["id"],
    to: edge.to as GraphDefinition["nodes"][number]["id"],
    type: edge.type,
    condition: edge.condition?.value
  }))
});
