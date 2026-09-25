import { describe, expect, it } from "vitest";

import { finalAnswer } from "./index.js";

describe("finalAnswer", () => {
  it("returns the text after the last final: marker", () => {
    expect(finalAnswer({ reasoning: "thought:check the order\nfinal:Refund issued." })).toBe("Refund issued.");
    expect(finalAnswer({ reasoning: "final:draft\nthought:no\nFINAL: done" })).toBe("done");
  });

  it("falls back to the whole reasoning, or an empty string", () => {
    expect(finalAnswer({ reasoning: "  plain answer " })).toBe("plain answer");
    expect(finalAnswer(undefined)).toBe("");
  });
});
