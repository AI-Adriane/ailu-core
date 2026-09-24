import { describe, expect, it } from "vitest";

import { MODEL_TIERS } from "@ailu/llm-gateway";

import { componentCatalog, prebuiltCatalog, tierCatalog } from "./index.js";
import { components } from "./components.js";
import { prebuilt } from "./prebuilt-agents.js";

/**
 * The catalog metadata is the SDK's source of truth for the API's `/catalog` endpoint.
 * We assert the counts and the tier vocabulary, and cross-check the catalog against the
 * live `components` / `prebuilt` surfaces so the metadata cannot drift from the
 * factories it describes.
 */
describe("@ailu/graph-sdk — catalog", () => {
  it("componentCatalog has 30 entries (28 pure + 2 integration)", () => {
    expect(componentCatalog.length).toBe(30);
    expect(componentCatalog.filter((c) => !c.integration).length).toBe(28);
    expect(componentCatalog.filter((c) => c.integration).length).toBe(2);
  });

  it("prebuiltCatalog has 16 entries", () => {
    expect(prebuiltCatalog.length).toBe(16);
  });

  it("tierCatalog has 4 tiers", () => {
    expect(tierCatalog.length).toBe(4);
  });

  it("every prebuilt tier is a valid ModelTier", () => {
    for (const entry of prebuiltCatalog) {
      expect(MODEL_TIERS).toContain(entry.tier);
    }
  });

  it("every component kind maps to a real factory on the components surface", () => {
    const factoryNames = new Set(Object.keys(components));
    for (const entry of componentCatalog) {
      expect(factoryNames.has(entry.kind)).toBe(true);
    }
    // And the catalog covers every factory exactly once.
    expect(componentCatalog.map((c) => c.kind).sort()).toEqual([...factoryNames].sort());
  });

  it("every prebuilt name maps to a real factory on the prebuilt surface", () => {
    const factoryNames = new Set(Object.keys(prebuilt));
    for (const entry of prebuiltCatalog) {
      expect(factoryNames.has(entry.name)).toBe(true);
    }
    expect(prebuiltCatalog.map((p) => p.name).sort()).toEqual([...factoryNames].sort());
  });

  it("only refundApprover suspends for approval and carries the refund tool", () => {
    const approving = prebuiltCatalog.filter((p) => p.suspendForApproval);
    expect(approving.map((p) => p.name)).toEqual(["refundApprover"]);
    expect(approving[0]?.tools).toEqual(["refund"]);
  });

  it("each tier carries the anthropic / mistral / ollama recommended models", () => {
    for (const tier of tierCatalog) {
      expect(MODEL_TIERS).toContain(tier.tier);
      expect(tier.models.anthropic).toBeTypeOf("string");
      expect(tier.models.mistral).toBeTypeOf("string");
      expect(tier.models.ollama).toBeTypeOf("string");
    }
    // The contract anchor: the fast tier resolves to haiku / mistral-small / mistral.
    const fast = tierCatalog.find((t) => t.tier === "fast");
    expect(fast?.models).toEqual({
      anthropic: "claude-haiku-4-5",
      mistral: "mistral-small-latest",
      ollama: "mistral"
    });
  });
});
