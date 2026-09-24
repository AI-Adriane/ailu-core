import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { resolveAgentModel, toRustAgentConfig } from "./agent-node.js";
import { DefaultLLMGateway } from "./index.js";

/**
 * Capability-tier resolution on the SDK surface. We exercise the TS fallback path's
 * `resolveAgentModel` (which mirrors the Rust `resolve_agent_model`) and confirm the
 * tier rides into the serializable agent config the Rust bridge consumes. No network:
 * resolution is pure over a forced environment.
 */
describe("@ailu/graph-sdk — capability tiers", () => {
  const PROVIDER_KEYS = ["ANTHROPIC_API_KEY", "MISTRAL_API_KEY", "AILU_USE_OLLAMA"] as const;
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    for (const key of PROVIDER_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(() => {
    for (const key of PROVIDER_KEYS) {
      if (saved[key] === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = saved[key];
      }
    }
  });

  it("resolves tier=fast to mistral-small-latest with ONLY Mistral available", () => {
    // Force the only-Mistral environment: the point of the contract — "I only have
    // Mistral" maps every tier to the mistral column.
    process.env.MISTRAL_API_KEY = "sk-mistral-test";

    const resolved = resolveAgentModel({ tier: "fast" });
    expect(resolved.provider).toBe("mistral");
    expect(resolved.model).toBe("mistral-small-latest");
  });

  it("resolves the other tiers to their mistral column on a mistral-only env", () => {
    process.env.MISTRAL_API_KEY = "sk-mistral-test";

    expect(resolveAgentModel({ tier: "frontier" }).model).toBe("mistral-large-latest");
    expect(resolveAgentModel({ tier: "balanced" }).model).toBe("mistral-medium-latest");
    expect(resolveAgentModel({ tier: "creative" }).model).toBe("mistral-large-latest");
  });

  it("resolves tiers to the anthropic column on an anthropic-only env", () => {
    process.env.ANTHROPIC_API_KEY = "sk-ant-test";

    expect(resolveAgentModel({ tier: "fast" })).toEqual({ provider: "anthropic", model: "claude-haiku-4-5" });
    expect(resolveAgentModel({ tier: "frontier" })).toEqual({
      provider: "anthropic",
      model: "claude-opus-4-8"
    });
    expect(resolveAgentModel({ tier: "creative" })).toEqual({
      provider: "anthropic",
      model: "claude-fable-5"
    });
  });

  it("lets an explicit model win over the tier (override precedence)", () => {
    process.env.MISTRAL_API_KEY = "sk-mistral-test";

    const resolved = resolveAgentModel({ tier: "fast", model: "pinned-model", provider: "anthropic" });
    expect(resolved).toEqual({ provider: "anthropic", model: "pinned-model" });
  });

  it("falls back to the mock provider when no provider is available", () => {
    // No keys set (cleared in beforeEach) and no override -> mock.
    const resolved = resolveAgentModel({ tier: "fast" });
    expect(resolved.provider).toBe("mock");
  });

  it("carries the tier into the serializable agent config for the Rust bridge", () => {
    const config = toRustAgentConfig("assistant", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "be helpful" },
      provider: "mistral",
      tier: "fast"
    });
    // The spec carries the abstract tier (not a resolved model) so the Rust bridge
    // resolves it from its own process env via ModelPolicy.
    expect(config.tier).toBe("fast");
    expect(config.model).toBeUndefined();
    expect(config.provider).toBe("mistral");
  });
});
