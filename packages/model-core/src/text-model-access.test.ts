import { describe, expect, it, vi } from "vitest";
import {
  assertTextModelAccessSupport,
  TextModelAccessUnsupportedError,
  type TextModelAccessSnapshot
} from "./index.js";

const policy: TextModelAccessSnapshot = {
  schemaVersion: 1,
  tenantId: "t",
  runId: "r",
  policyId: "organization-text-models",
  policyVersion: 1,
  catalogRevision: "catalog",
  policyHash: "a".repeat(64),
  allowedModels: []
};

describe("native text-model capability handshake", () => {
  it("refuses a governed call on an older or incompatible addon", () => {
    for (const native of [
      {},
      { textModelAccessVersion: () => 0 },
      { textModelAccessVersion: () => 2 }
    ]) {
      expect(() => assertTextModelAccessSupport(native, policy)).toThrow(
        TextModelAccessUnsupportedError
      );
    }
    expect(() =>
      assertTextModelAccessSupport({ textModelAccessVersion: () => 1 }, policy)
    ).not.toThrow();
  });
  it("preserves legacy calls without requiring or invoking the new native capability", () => {
    const capability = vi.fn(() => {
      throw new Error("must not be consulted");
    });
    expect(() =>
      assertTextModelAccessSupport({ textModelAccessVersion: capability }, undefined)
    ).not.toThrow();
    expect(capability).not.toHaveBeenCalled();
    expect(() => assertTextModelAccessSupport({}, undefined)).not.toThrow();
  });
});
