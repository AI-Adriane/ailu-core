import { describe, expect, it } from "vitest";

import { CatalogDtoSchema, type CatalogDto } from "./catalog.js";

/**
 * The catalog contract is the API↔Studio boundary for the building-block library.
 * We assert a representative `CatalogDto` parses cleanly and round-trips unchanged, so
 * the API can validate the SDK metadata against this schema and forward it.
 */
describe("@ailu-ai/contracts — catalog", () => {
  const sample: CatalogDto = {
    components: [
      {
        kind: "promptBuilder",
        title: "Prompt Builder",
        category: "prompt",
        description: "Render {{var}} placeholders from the channels into a target channel.",
        params: [
          { name: "template", type: "string", required: true, description: "Template with {{var}} placeholders." },
          { name: "into", type: "string", required: true, description: "Channel the rendered string is written into." }
        ],
        integration: false
      },
      {
        kind: "httpFetch",
        title: "HTTP Fetch",
        category: "integration",
        description: "Fetch a URL via an injectable fetch impl.",
        params: [
          { name: "into", type: "string", required: true, description: "Channel receiving the result." }
        ],
        integration: true
      }
    ],
    prebuilt: [
      {
        name: "summarizer",
        title: "Summarizer",
        description: "Condenses input text into a short, faithful summary.",
        tier: "fast",
        tools: [],
        suspendForApproval: false,
        outputChannel: "summary"
      },
      {
        name: "refundApprover",
        title: "Refund Approver",
        description: "Decides whether to issue a refund behind a human approval gate.",
        tier: "balanced",
        tools: ["refund"],
        suspendForApproval: true,
        outputChannel: "refundDecision"
      }
    ],
    tiers: [
      {
        tier: "fast",
        description: "Lowest-latency, lowest-cost models for high-volume simple tasks.",
        models: { anthropic: "claude-haiku-4-5", mistral: "mistral-small-latest", ollama: "mistral" }
      }
    ],
    exampleGraphs: [
      {
        slug: "publish-flow",
        name: "Publish flow",
        description: "A human-in-the-loop flow: write, approval gate, publish.",
        definition: {
          version: "1.0.0",
          name: "publish-flow",
          channels: {
            draft: { type: "string", reducer: "replace", default: "" }
          },
          nodes: [
            { id: "write", type: "action", label: "write" },
            { id: "review", type: "human-gate", label: "review" }
          ],
          edges: [{ id: "write->review", from: "write", to: "review", type: "default" }],
          entryNodeId: "write"
        }
      }
    ]
  };

  it("parses a representative CatalogDto", () => {
    const result = CatalogDtoSchema.safeParse(sample);
    expect(result.success).toBe(true);
  });

  it("round-trips a CatalogDto through parse unchanged", () => {
    const parsed = CatalogDtoSchema.parse(sample);
    expect(parsed).toEqual(sample);
  });

  it("rejects an invalid tier", () => {
    const bad = { ...sample, prebuilt: [{ ...sample.prebuilt[0], tier: "ultra" }] };
    expect(CatalogDtoSchema.safeParse(bad).success).toBe(false);
  });

  it("rejects an unknown component category", () => {
    const bad = { ...sample, components: [{ ...sample.components[0], category: "magic" }] };
    expect(CatalogDtoSchema.safeParse(bad).success).toBe(false);
  });
});
