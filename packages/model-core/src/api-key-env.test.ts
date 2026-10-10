import { describe, expect, it } from "vitest";

import {
  API_KEY_ENV_ALLOWLIST_ENV,
  apiKeyEnvAllowed,
  parseApiKeyEnvAllowlist
} from "./api-key-env.js";

describe("the apiKeyEnv allow-list matcher", () => {
  it("is read from AILU_API_KEY_ENV_ALLOWLIST, the variable the engine reads", () => {
    expect(API_KEY_ENV_ALLOWLIST_ENV).toBe("AILU_API_KEY_ENV_ALLOWLIST");
  });

  const allowed = (raw: string, name: string): boolean =>
    apiKeyEnvAllowed(parseApiKeyEnvAllowlist(raw), name);

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
});
