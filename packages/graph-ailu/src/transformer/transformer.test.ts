import { describe, expect, it } from "vitest";

import { buildGraphAST } from "../parser/build-graph-ast.js";
import { transformGraph } from "./transform-graph.js";

describe("transformGraph", () => {
  it("maps channel reducer correctly", () => {
    const ast = buildGraphAST(
      {
        id: "g",
        version: "1.0.0",
        name: "graph",
        entryNodeId: "n1",
        channels: {
          ctx: { type: "object", reducer: "merge", default: {} }
        },
        nodes: [{ id: "n1", type: "action", label: "N1" }],
        edges: []
      },
      "graph.yaml"
    );
    const def = transformGraph(ast);
    expect(def.channels.ctx?.reducer).toBe("merge");
  });
});
