import { describe, expect, it } from "vitest";

import { buildGraphAST } from "./build-graph-ast.js";

describe("buildGraphAST", () => {
  it("parses subgraph versioned reference", () => {
    const ast = buildGraphAST(
      {
        id: "g",
        version: "1.0.0",
        name: "x",
        entryNodeId: "s1",
        channels: {},
        nodes: [{ id: "s1", type: "subgraph", label: "sub", graph: "risk-agent@1.0.0" }],
        edges: []
      },
      "graph.yaml"
    );
    expect(ast.nodes[0]?.subgraph).toEqual({ id: "risk-agent", version: "1.0.0" });
  });
});
