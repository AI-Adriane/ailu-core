import { describe, expect, it } from "vitest";

import { buildGraphAST } from "../parser/build-graph-ast.js";
import { validateGraphAST } from "./validate-graph-ast.js";

describe("validateGraphAST", () => {
  it("returns error with loc when edge references missing node", () => {
    const ast = buildGraphAST(
      {
        id: "g",
        version: "1.0.0",
        name: "graph",
        entryNodeId: "n1",
        channels: {},
        nodes: [{ id: "n1", type: "action", label: "N1" }],
        edges: [{ id: "e1", from: "n1", to: "missing", type: "default" }]
      },
      "graph.yaml"
    );
    const diagnostics = validateGraphAST(ast);
    const missing = diagnostics.find((d) => d.code === "EDGE_NODE_NOT_FOUND");
    expect(missing).toBeDefined();
    expect(missing?.loc.file).toBe("graph.yaml");
  });
});
