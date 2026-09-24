import { describe, expect, it } from "vitest";

import { gemini, GeminiModel } from "./index.js";

describe("@ailu/model-gemini", () => {
  it("gemini(id) and new GeminiModel(id) declare the google provider", () => {
    expect(gemini("m").toSpec()).toMatchObject({ provider: "google", model: "m" });
    expect(new GeminiModel("m").toSpec().provider).toBe("google");
  });

  it("tier shortcuts set the tier and leave the model for the engine to resolve", () => {
    const spec = gemini.frontier().toSpec();
    expect(spec).toMatchObject({ provider: "google", tier: "frontier" });
    expect(spec.model).toBeUndefined();
  });
});
