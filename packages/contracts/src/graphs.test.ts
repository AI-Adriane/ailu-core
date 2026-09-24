import { describe, expect, it } from "vitest";

import { ExampleGraphDtoSchema, type ExampleGraphDto } from "./graphs.js";

/**
 * The example-graph contract is the API↔Studio boundary for SDK-authored example
 * graphs. We assert a representative `ExampleGraphDto` parses cleanly and round-trips
 * unchanged, so the API can validate the SDK metadata against this schema and forward
 * it, and the Studio can render the preview + create flow from plain data.
 */
describe("@ailu-ai/contracts — example graphs", () => {
  const sample: ExampleGraphDto = {
    slug: "publish-flow",
    name: "Publish flow",
    description: "A human-in-the-loop flow: write, approval gate, publish.",
    definition: {
      version: "1.0.0",
      name: "publish-flow",
      channels: {
        draft: { type: "string", reducer: "replace", default: "" },
        approved: { type: "boolean", reducer: "replace", default: false }
      },
      nodes: [
        { id: "write", type: "action", label: "write" },
        { id: "review", type: "human-gate", label: "review" },
        { id: "publish", type: "action", label: "publish" }
      ],
      edges: [
        { id: "write->review", from: "write", to: "review", type: "default" },
        { id: "review->publish", from: "review", to: "publish", type: "default" }
      ],
      entryNodeId: "write"
    }
  };

  it("parses a representative ExampleGraphDto", () => {
    const result = ExampleGraphDtoSchema.safeParse(sample);
    expect(result.success).toBe(true);
  });

  it("round-trips an ExampleGraphDto through parse unchanged", () => {
    const parsed = ExampleGraphDtoSchema.parse(sample);
    expect(parsed).toEqual(sample);
  });

  it("accepts an optional id on the definition", () => {
    const withId: ExampleGraphDto = {
      ...sample,
      definition: { ...sample.definition, id: "graph_123" }
    };
    expect(ExampleGraphDtoSchema.safeParse(withId).success).toBe(true);
  });

  it("rejects an unknown node type", () => {
    const bad = {
      ...sample,
      definition: {
        ...sample.definition,
        nodes: [{ id: "write", type: "magic", label: "write" }]
      }
    };
    expect(ExampleGraphDtoSchema.safeParse(bad).success).toBe(false);
  });

  it("rejects a definition missing its entry node id", () => {
    const definition: Record<string, unknown> = { ...sample.definition };
    delete definition.entryNodeId;
    const bad = { ...sample, definition };
    expect(ExampleGraphDtoSchema.safeParse(bad).success).toBe(false);
  });
});
