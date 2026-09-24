import { describe, expect, it } from "vitest";

import { compileGraphFile } from "./compile-graph-file.js";

describe("compileGraphFile", () => {
  it("compiles a valid graph", () => {
    const compiled = compileGraphFile(
      `
id: graph-1
version: 1.0.0
name: Demo graph
entryNodeId: n1
channels:
  messages:
    type: messages
    reducer: append
nodes:
  - id: n1
    type: action
    label: Start
edges: []
`,
      "graph.yaml"
    );
    expect(compiled.result).toBeDefined();
    expect(compiled.diagnostics.filter((d) => d.severity === "error")).toHaveLength(0);
  });
});
