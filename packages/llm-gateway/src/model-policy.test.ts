import { describe, expect, it } from "vitest";

import {
  DEFAULT_PREFERENCE,
  MOCK_MODEL,
  ModelPolicy,
  type ModelTier
} from "./model-policy.js";
import type { LLMProvider } from "./types.js";

describe("ModelPolicy", () => {
  it("only-mistral routes every tier to the mistral column", () => {
    const policy = new ModelPolicy();
    const available: LLMProvider[] = ["mistral"];

    const frontier = policy.resolve("frontier", available);
    const balanced = policy.resolve("balanced", available);
    const fast = policy.resolve("fast", available);
    const creative = policy.resolve("creative", available);

    expect(frontier).toEqual({
      provider: "mistral",
      model: "mistral-large-latest",
      recommended: true
    });
    expect(balanced.model).toBe("mistral-medium-latest");
    expect(fast.model).toBe("mistral-small-latest");
    expect(creative.model).toBe("mistral-large-latest");
    for (const choice of [frontier, balanced, fast, creative]) {
      expect(choice.provider).toBe("mistral");
      expect(choice.recommended).toBe(true);
    }
  });

  it("only-anthropic maps each tier to its claude model", () => {
    const policy = new ModelPolicy();
    const available: LLMProvider[] = ["anthropic"];

    expect(policy.resolve("fast", available).model).toBe("claude-haiku-4-5");
    expect(policy.resolve("frontier", available).model).toBe("claude-opus-4-8");
    expect(policy.resolve("creative", available).model).toBe("claude-fable-5");
    expect(policy.resolve("balanced", available).model).toBe("claude-sonnet-4-6");

    const fast = policy.resolve("fast", available);
    expect(fast.provider).toBe("anthropic");
    expect(fast.recommended).toBe(true);
  });

  it("override model wins and is not recommended", () => {
    const policy = new ModelPolicy();
    const available: LLMProvider[] = ["anthropic", "mistral"];

    const choice = policy.resolve("frontier", available, { model: "my-custom-model" });

    // Provider falls back to the first available (anthropic), model is the
    // override, and recommended is false.
    expect(choice).toEqual({
      provider: "anthropic",
      model: "my-custom-model",
      recommended: false
    });
  });

  it("override provider and model both win", () => {
    const policy = new ModelPolicy();

    const choice = policy.resolve("fast", ["anthropic"], {
      provider: "mistral",
      model: "mistral-tiny"
    });

    expect(choice).toEqual({
      provider: "mistral",
      model: "mistral-tiny",
      recommended: false
    });
  });

  it("override provider alone maps to that provider's tier model", () => {
    const policy = new ModelPolicy();

    const choice = policy.resolve("fast", ["anthropic", "mistral"], { provider: "mistral" });

    expect(choice).toEqual({
      provider: "mistral",
      model: "mistral-small-latest",
      recommended: false
    });
  });

  it("preference order picks anthropic over mistral when both present", () => {
    const policy = new ModelPolicy();
    const available: LLMProvider[] = ["mistral", "anthropic"];

    const choice = policy.resolve("balanced", available);

    expect(choice).toEqual({
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      recommended: true
    });
  });

  it("no provider available resolves to mock", () => {
    const policy = new ModelPolicy();

    const choice = policy.resolve("frontier", []);

    expect(choice).toEqual({
      provider: "mock",
      model: MOCK_MODEL,
      recommended: false
    });
  });

  it("serialises the tier as the camelCase wire string", () => {
    // ModelTier is the literal union itself, so JSON round-trips it verbatim —
    // this is the TS analogue of the Rust serde camelCase round-trip test.
    const tiers: ModelTier[] = ["frontier", "balanced", "fast", "creative"];
    for (const tier of tiers) {
      const json = JSON.stringify({ tier });
      expect(json).toContain(`"tier":"${tier}"`);
      const parsed = JSON.parse(json) as { tier: ModelTier };
      expect(parsed.tier).toBe(tier);
    }
  });

  it("availableFromEnv reads keys and follows the preference order", () => {
    const policy = new ModelPolicy();

    expect(
      policy.availableFromEnv({
        ANTHROPIC_API_KEY: "sk-ant",
        MISTRAL_API_KEY: "sk-mistral",
        AILU_USE_OLLAMA: "1"
      })
    ).toEqual(["anthropic", "mistral", "ollama"]);

    expect(policy.availableFromEnv({ MISTRAL_API_KEY: "sk-mistral" })).toEqual(["mistral"]);

    expect(policy.availableFromEnv({})).toEqual([]);

    // An empty string is not a present key.
    expect(policy.availableFromEnv({ ANTHROPIC_API_KEY: "" })).toEqual([]);

    // AILU_USE_OLLAMA must be exactly "1".
    expect(policy.availableFromEnv({ AILU_USE_OLLAMA: "true" })).toEqual([]);
  });

  it("only-mistral env routes every tier to the mistral column end-to-end", () => {
    const policy = new ModelPolicy();
    const available = policy.availableFromEnv({ MISTRAL_API_KEY: "sk-mistral" });

    expect(available).toEqual(["mistral"]);
    expect(policy.resolve("frontier", available).model).toBe("mistral-large-latest");
    expect(policy.resolve("fast", available).model).toBe("mistral-small-latest");
  });

  it("custom overrides replace the table and preference", () => {
    const policy = new ModelPolicy({
      table: {
        mistral: {
          frontier: "frontier-x",
          balanced: "balanced-x",
          fast: "fast-x",
          creative: "creative-x"
        }
      },
      preference: ["mistral"]
    });

    const choice = policy.resolve("fast", ["mistral"]);
    expect(choice).toEqual({ provider: "mistral", model: "fast-x", recommended: true });
  });

  it("exposes the contract preference order", () => {
    expect([...DEFAULT_PREFERENCE]).toEqual(["anthropic", "mistral", "ollama"]);
  });
});
