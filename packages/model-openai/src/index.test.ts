import { describe, expect, it } from "vitest";

import { openai, OpenAIModel } from "./index.js";

describe("@ailu-ai/model-openai", () => {
  it("openai(id) and new OpenAIModel(id) declare the openai provider", () => {
    expect(openai("gpt-4o").toSpec()).toMatchObject({ provider: "openai", model: "gpt-4o" });
    expect(new OpenAIModel("gpt-4o").toSpec().provider).toBe("openai");
  });

  it("tier shortcuts set the tier and leave the model for the engine to resolve", () => {
    const m = openai.frontier().toSpec();
    expect(m).toMatchObject({ provider: "openai", tier: "frontier" });
    expect(m.model).toBeUndefined();
  });
});
