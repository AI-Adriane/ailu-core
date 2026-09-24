import { describe, expect, it } from "vitest";

import {
  FsResolvedPolicyDtoSchema,
  SetFsPolicyDtoSchema,
  FsPathRuleSchema
} from "./fs-policy.js";

/**
 * The fs policy contract is the API↔Studio boundary for the owner-only per-path
 * permission rules (ADR 0024 phase 2d). Verbs are a closed set; a resolved policy
 * carries server-computed capability flags.
 */
describe("@ailu/contracts — fs-policy", () => {
  it("parses a set-policy body with valid rules", () => {
    const body = SetFsPolicyDtoSchema.parse({
      rules: [
        { glob: "scratch/**", verb: "write" },
        { glob: "review/**", verb: "gate" },
        { glob: "secret/**", verb: "deny" }
      ]
    });
    expect(body.rules).toHaveLength(3);
    expect(body.rules[1]?.verb).toBe("gate");
  });

  it("rejects an unknown verb", () => {
    expect(FsPathRuleSchema.safeParse({ glob: "x/**", verb: "execute" }).success).toBe(false);
  });

  it("parses a resolved policy with capability flags", () => {
    const resolved = FsResolvedPolicyDtoSchema.parse({
      path: "review/doc.md",
      verb: "gate",
      canRead: true,
      canWrite: true,
      requiresGate: true
    });
    expect(resolved.requiresGate).toBe(true);
    expect(resolved.canWrite).toBe(true);
  });
});
