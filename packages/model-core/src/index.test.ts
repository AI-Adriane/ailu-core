import { describe, expect, it } from "vitest";

import {
  assertKnownProvider,
  model,
  models,
  Model,
  MissingProviderKeyError,
  NoProviderInEnvError,
  openaiCompatible,
  parseModelString,
  resolveProviderKeys,
  toModelSpec,
  UnknownProviderError,
  type ModelSpec
} from "./index.js";

class TestModel extends Model {
  readonly spec: ModelSpec = { provider: "openai", model: "x" };
}

describe("@ailu-ai/model-core", () => {
  it("toSpec / toJSON return the plain spec", () => {
    const m = new TestModel();
    expect(m.toSpec()).toEqual({ provider: "openai", model: "x" });
    expect(JSON.parse(JSON.stringify(m))).toEqual({ provider: "openai", model: "x" });
  });

  it("toModelSpec normalizes a Model, a bare spec, or a string", () => {
    expect(toModelSpec(new TestModel())).toEqual({ provider: "openai", model: "x" });
    expect(toModelSpec({ provider: "anthropic" })).toEqual({ provider: "anthropic" });
    expect(toModelSpec("openai:gpt-4o")).toEqual({ provider: "openai", model: "gpt-4o" });
    expect(toModelSpec("anthropic:frontier")).toEqual({ provider: "anthropic", tier: "frontier" });
    expect(toModelSpec("mistral")).toEqual({ provider: "mistral" });
  });

  it("parseModelString fails loud on an unknown provider", () => {
    expect(() => parseModelString("cohere:x")).toThrow(UnknownProviderError);
  });

  it("assertKnownProvider fails loudly on an unknown slug", () => {
    expect(() => assertKnownProvider("nope")).toThrow(UnknownProviderError);
    expect(() => assertKnownProvider("openai")).not.toThrow();
  });

  it("openaiCompatible builds an openai-slug spec carrying baseURL", () => {
    const spec = openaiCompatible("llama-3", { baseURL: "http://localhost:1234/v1" }).toSpec();
    expect(spec.provider).toBe("openai");
    expect(spec.model).toBe("llama-3");
    expect(spec.baseURL).toBe("http://localhost:1234/v1");
  });

  describe("the model surface (ADR 0034)", () => {
    it("provider methods build a concrete spec; tiers are model-valued properties", () => {
      expect(model.openai("gpt-4o").toSpec()).toEqual({ provider: "openai", model: "gpt-4o" });
      expect(model.anthropic.frontier.toSpec()).toEqual({ provider: "anthropic", tier: "frontier" });
      expect(model.gemini("gemini-2.5-pro").toSpec().provider).toBe("google"); // gemini → google slug
      expect(model.fast.toSpec()).toEqual({ tier: "fast" }); // provider-less: resolved from env
    });

    it("object form and string form both normalize", () => {
      expect(model({ provider: "openai", tier: "fast" }).toSpec()).toEqual({
        provider: "openai",
        tier: "fast"
      });
      expect(model("openai:fast").toSpec()).toEqual({ provider: "openai", tier: "fast" });
    });

    it("openaiCompatible escape hatch carries the endpoint", () => {
      const spec = model
        .openaiCompatible({ baseURL: "http://localhost:1234/v1", model: "qwen2.5" })
        .toSpec();
      expect(spec).toMatchObject({ provider: "openai", model: "qwen2.5", baseURL: "http://localhost:1234/v1" });
    });

    it("models is an alias of model", () => {
      expect(models).toBe(model);
    });

    it(".output() attaches a schema without mutating the base spec", () => {
      const base = model.openai("gpt-4o");
      const typed = base.output<{ ok: boolean }>({
        jsonSchema: { type: "object", properties: { ok: { type: "boolean" } } },
        parse: (v) => v as { ok: boolean }
      });
      expect(typed.toSpec()).toEqual({ provider: "openai", model: "gpt-4o" }); // spec unchanged
    });
  });

  describe("resolveProviderKeys (ADR 0034, env-injected)", () => {
    it("explicit provider with its key present → that key", () => {
      const r = resolveProviderKeys({ provider: "openai" }, { OPENAI_API_KEY: "sk-x" });
      expect(r).toEqual({ provider: "openai", providerKeys: { openai: "sk-x" } });
    });

    it("explicit provider, key absent → MissingProviderKeyError naming the var", () => {
      expect(() => resolveProviderKeys({ provider: "openai" }, {})).toThrow(MissingProviderKeyError);
      try {
        resolveProviderKeys({ provider: "anthropic" }, {});
      } catch (e) {
        expect((e as MissingProviderKeyError).envVar).toBe("ANTHROPIC_API_KEY");
      }
    });

    it("keyless provider (ollama) needs no key", () => {
      expect(resolveProviderKeys({ provider: "ollama" }, {})).toEqual({
        provider: "ollama",
        providerKeys: {}
      });
    });

    it("provider-less: picks the highest-preference provider whose key is present", () => {
      const r = resolveProviderKeys({ tier: "fast" }, { OPENAI_API_KEY: "sk-x" });
      expect(r.provider).toBe("openai");
      // anthropic outranks openai in preference order when both are present:
      const r2 = resolveProviderKeys({ tier: "fast" }, { OPENAI_API_KEY: "a", ANTHROPIC_API_KEY: "b" });
      expect(r2.provider).toBe("anthropic");
    });

    it("provider-less with no keys → NoProviderInEnvError", () => {
      expect(() => resolveProviderKeys({ tier: "balanced" }, {})).toThrow(NoProviderInEnvError);
    });

    it("a custom baseURL never receives the provider's default key", () => {
      const env = { OPENAI_API_KEY: "sk-public-openai", VLLM_KEY: "vllm-secret" };
      // keyless endpoint: OPENAI_API_KEY is present but must not be picked up
      expect(
        resolveProviderKeys(
          { provider: "openai", model: "llama", baseURL: "http://vllm.internal/v1" },
          env
        )
      ).toEqual({ provider: "openai", providerKeys: {} });
      // keyed endpoint: only the variable named by apiKeyEnv
      expect(
        resolveProviderKeys(
          { provider: "openai", baseURL: "http://vllm.internal/v1", apiKeyEnv: "VLLM_KEY" },
          env
        )
      ).toEqual({ provider: "openai", providerKeys: { openai: "vllm-secret" } });
      expect(() =>
        resolveProviderKeys({ baseURL: "http://vllm.internal/v1", apiKeyEnv: "MISSING_KEY" }, env)
      ).toThrow(MissingProviderKeyError);
    });

    it("apiKeyEnv overrides the default env var", () => {
      const r = resolveProviderKeys({ provider: "openai", apiKeyEnv: "CORP_KEY" }, { CORP_KEY: "k" });
      expect(r.providerKeys).toEqual({ openai: "k" });
    });

    it("reads the same key aliases as the engine (GOOGLE_API_KEY, HUGGINGFACE_API_KEY)", () => {
      expect(resolveProviderKeys({ provider: "google" }, { GOOGLE_API_KEY: "g" }).providerKeys).toEqual({
        google: "g"
      });
      expect(resolveProviderKeys({ provider: "huggingface" }, { HF_TOKEN: "h" }).providerKeys).toEqual({
        huggingface: "h"
      });
      expect(
        resolveProviderKeys({ provider: "huggingface" }, { HUGGINGFACE_API_KEY: "h2" }).providerKeys
      ).toEqual({ huggingface: "h2" });
      expect(resolveProviderKeys({ tier: "fast" }, { GOOGLE_API_KEY: "g" }).provider).toBe("google");
    });

    it("offline mode (AILU_LLM_MOCK=1) turns a missing key into a keyless call", () => {
      const offline = { AILU_LLM_MOCK: "1" };
      expect(resolveProviderKeys({ provider: "openai" }, offline)).toEqual({ provider: "openai", providerKeys: {} });
      expect(resolveProviderKeys({ tier: "fast" }, offline).providerKeys).toEqual({});
      // A key that IS set still wins.
      expect(resolveProviderKeys({ provider: "openai" }, { ...offline, OPENAI_API_KEY: "k" }).providerKeys).toEqual({
        openai: "k"
      });
    });
  });
});
