import { describe, expect, it } from "vitest";

import { mistral, MistralModel } from "./index.js";

describe("@ailu-ai/model-mistral", () => {
  it("mistral(id) and new MistralModel(id) declare the mistral provider", () => {
    expect(mistral("m").toSpec()).toMatchObject({ provider: "mistral", model: "m" });
    expect(new MistralModel("m").toSpec().provider).toBe("mistral");
  });

  it("tier shortcuts set the tier and leave the model for the engine to resolve", () => {
    const spec = mistral.frontier().toSpec();
    expect(spec).toMatchObject({ provider: "mistral", tier: "frontier" });
    expect(spec.model).toBeUndefined();
  });
});
