import { describe, expect, it } from "vitest";

import {
  API_KEY_ENV_ALLOWLIST_ENV,
  ApiKeyEnvNotAllowedError,
  apiKeyEnvAllowed,
  assertKnownProvider,
  model,
  models,
  Model,
  MissingProviderKeyError,
  NoProviderInEnvError,
  openaiCompatible,
  parseApiKeyEnvAllowlist,
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

  describe("the apiKeyEnv allow-list (AILU_API_KEY_ENV_ALLOWLIST)", () => {
    const allowed = (raw: string, name: string): boolean =>
      apiKeyEnvAllowed(parseApiKeyEnvAllowlist(raw), name);

    it("is read from AILU_API_KEY_ENV_ALLOWLIST, the variable the engine reads", () => {
      expect(API_KEY_ENV_ALLOWLIST_ENV).toBe("AILU_API_KEY_ENV_ALLOWLIST");
    });

    it("matches exact names, case-sensitively", () => {
      expect(allowed("GATEWAY_KEY,VLLM_KEY", "GATEWAY_KEY")).toBe(true);
      expect(allowed("GATEWAY_KEY,VLLM_KEY", "VLLM_KEY")).toBe(true);
      expect(allowed("GATEWAY_KEY", "GATEWAY_KEY_2")).toBe(false);
      expect(allowed("GATEWAY_KEY", "GATEWAY")).toBe(false);
      expect(allowed("GATEWAY_KEY", "gateway_key")).toBe(false);
    });

    it("treats a trailing * as a prefix, and only a trailing one", () => {
      expect(allowed("AILU_ENDPOINT_*", "AILU_ENDPOINT_VLLM")).toBe(true);
      expect(allowed("AILU_ENDPOINT_*", "AILU_ENDPOINT_")).toBe(true);
      expect(allowed("AILU_ENDPOINT_*", "AILU_ENDPOINT")).toBe(false);
      expect(allowed("AILU_ENDPOINT_*", "OPENAI_API_KEY")).toBe(false);
      expect(allowed("AILU_*_KEY", "AILU_VLLM_KEY")).toBe(false);
    });

    it("trims entries and ignores blank ones; an empty list allows none", () => {
      expect(allowed(" GATEWAY_KEY , ,AILU_ENDPOINT_* ,", "GATEWAY_KEY")).toBe(true);
      expect(allowed(" GATEWAY_KEY , ,AILU_ENDPOINT_* ,", "AILU_ENDPOINT_X")).toBe(true);
      for (const raw of ["", "   ", ",", " , "]) {
        expect(allowed(raw, "GATEWAY_KEY")).toBe(false);
        expect(allowed(raw, "")).toBe(false);
      }
    });

    it("unset: every apiKeyEnv is read as before", () => {
      expect(
        resolveProviderKeys(
          { baseURL: "http://gw.internal/v1", apiKeyEnv: "MY_GATEWAY_KEY" },
          { MY_GATEWAY_KEY: "k" }
        ).providerKeys
      ).toEqual({ openai: "k" });
    });

    it("set: a custom endpoint's apiKeyEnv outside the list is refused, set or not", () => {
      const env = {
        [API_KEY_ENV_ALLOWLIST_ENV]: "AILU_ENDPOINT_*",
        DATABASE_URL: "postgres://user:hunter2@db",
        AILU_ENDPOINT_VLLM: "vllm-secret"
      };
      const refuse = (apiKeyEnv: string): ApiKeyEnvNotAllowedError => {
        try {
          resolveProviderKeys({ baseURL: "http://vllm.internal/v1", apiKeyEnv }, env);
        } catch (error) {
          if (error instanceof ApiKeyEnvNotAllowedError) return error;
          throw error;
        }
        throw new Error(`expected ${apiKeyEnv} to be refused`);
      };
      const refused = refuse("DATABASE_URL");
      expect(refused.code).toBe("AILU_API_KEY_ENV_NOT_ALLOWED");
      expect(refused.envVar).toBe("DATABASE_URL");
      expect(refused.message).toContain("DATABASE_URL");
      expect(refused.message).toContain("AILU_API_KEY_ENV_ALLOWLIST");
      expect(refused.message).not.toContain("hunter2");
      expect(refused.message).not.toContain("AILU_ENDPOINT_*");
      expect(refuse("AILU_TEST_NOT_SET").envVar).toBe("AILU_TEST_NOT_SET");
      // An allowed name is read; a keyless endpoint needs no entry.
      const endpoint = (apiKeyEnv?: string): ModelSpec => ({
        baseURL: "http://vllm.internal/v1",
        apiKeyEnv
      });
      expect(resolveProviderKeys(endpoint("AILU_ENDPOINT_VLLM"), env).providerKeys).toEqual({
        openai: "vllm-secret"
      });
      expect(resolveProviderKeys(endpoint(), env).providerKeys).toEqual({});
      // Allowed but missing is still a missing key.
      expect(() => resolveProviderKeys(endpoint("AILU_ENDPOINT_NONE"), env)).toThrow(
        MissingProviderKeyError
      );
    });

    it("set to empty: no apiKeyEnv is allowed", () => {
      expect(() =>
        resolveProviderKeys(
          { baseURL: "http://vllm.internal/v1", apiKeyEnv: "VLLM_KEY" },
          { [API_KEY_ENV_ALLOWLIST_ENV]: "", VLLM_KEY: "v" }
        )
      ).toThrow(ApiKeyEnvNotAllowedError);
    });

    it("set: a named provider's apiKeyEnv follows the rule; its default variable does not", () => {
      const env = {
        [API_KEY_ENV_ALLOWLIST_ENV]: "CORP_*",
        CORP_KEY: "k",
        HOST_SECRET: "s",
        OPENAI_API_KEY: "o"
      };
      const openai = (apiKeyEnv?: string): ModelSpec => ({ provider: "openai", apiKeyEnv });
      expect(resolveProviderKeys(openai("CORP_KEY"), env).providerKeys).toEqual({ openai: "k" });
      expect(() => resolveProviderKeys(openai("HOST_SECRET"), env)).toThrow(
        ApiKeyEnvNotAllowedError
      );
      // The provider's own variable is not an apiKeyEnv: the list does not apply to it.
      expect(resolveProviderKeys(openai(), env).providerKeys).toEqual({ openai: "o" });
    });
  });
});
