import assert from "node:assert/strict";

// #region example
import { compileGraphFile, validateGraph } from "@ailu-ai/graph-sdk";

const yaml = `
id: triage
version: 1.0.0
name: Ticket triage
entryNodeId: intake
channels:
  ticket: { type: string, reducer: replace, default: "" }
nodes:
  - { id: intake, type: action, label: Intake }
  - { id: review, type: human-gate, label: Human review }
edges:
  - { id: e1, from: intake, to: review, type: default }
`;

// YAML describes the graph's shape. Compile it to the same GraphDefinition the builder produces.
const { result, diagnostics } = compileGraphFile(yaml, "triage.graph.yaml");
console.log(diagnostics); // [] when the file is valid
console.log(validateGraph(result!)); // the engine's own validation
// #endregion example

assert.ok(result !== undefined);
assert.equal(diagnostics.filter((d) => d.severity === "error").length, 0);
