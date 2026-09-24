import { describe, expect, it } from "vitest";

import { anthropic, AnthropicModel } from "./index.js";

describe("@ailu/model-anthropic", () => {
  it("anthropic(id) and new AnthropicModel(id) declare the anthropic provider", () => {
    expect(anthropic("m").toSpec()).toMatchObject({ provider: "anthropic", model: "m" });
    expect(new AnthropicModel("m").toSpec().provider).toBe("anthropic");
  });

  it("tier shortcuts set the tier and leave the model for the engine to resolve", () => {
    const spec = anthropic.frontier().toSpec();
    expect(spec).toMatchObject({ provider: "anthropic", tier: "frontier" });
    expect(spec.model).toBeUndefined();
  });
});
