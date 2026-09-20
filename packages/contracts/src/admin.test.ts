import { describe, expect, it } from "vitest";

import { LlmProviderSchema, SetLlmProviderKeyDtoSchema } from "./admin.js";

describe("LlmProviderSchema", () => {
  it("includes the keyless local providers", () => {
    expect(() => LlmProviderSchema.parse("ollama")).not.toThrow();
    expect(() => LlmProviderSchema.parse("lmstudio")).not.toThrow();
  });

  it("rejects an unknown provider", () => {
    expect(() => LlmProviderSchema.parse("not-a-provider")).toThrow();
  });
});

describe("SetLlmProviderKeyDtoSchema — keyless local providers", () => {
  it("accepts a body with only a baseUrl (the Ollama case)", () => {
    // A local server needs no credential; requiring apiKey made `ollama` impossible to configure.
    expect(() =>
      SetLlmProviderKeyDtoSchema.parse({ baseUrl: "http://127.0.0.1:11434/v1" })
    ).not.toThrow();
  });

  it("accepts a baseUrl plus a default model", () => {
    expect(() =>
      SetLlmProviderKeyDtoSchema.parse({
        baseUrl: "http://127.0.0.1:11434/v1",
        defaultModel: "llama3.1"
      })
    ).not.toThrow();
  });

  it("still rejects an EMPTY apiKey when one is supplied", () => {
    // Optional must not become "anything goes": a cloud provider can never store an empty secret.
    expect(() => SetLlmProviderKeyDtoSchema.parse({ apiKey: "" })).toThrow();
    expect(() =>
      SetLlmProviderKeyDtoSchema.parse({ apiKey: "", baseUrl: "https://api.openai.com/v1" })
    ).toThrow();
  });

  it("still accepts the cloud-provider shape it always accepted", () => {
    const parsed = SetLlmProviderKeyDtoSchema.parse({
      apiKey: "sk-live-secret",
      baseUrl: "https://openrouter.ai/api/v1",
      defaultModel: "anthropic/claude-3.5-sonnet"
    });
    expect(parsed.apiKey).toBe("sk-live-secret");
    expect(parsed.baseUrl).toBe("https://openrouter.ai/api/v1");
  });

  it("leaves apiKey undefined rather than defaulting it", () => {
    const parsed = SetLlmProviderKeyDtoSchema.parse({ baseUrl: "http://127.0.0.1:11434/v1" });
    expect(parsed.apiKey).toBeUndefined();
  });
});
