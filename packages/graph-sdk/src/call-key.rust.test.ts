import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { callInputOf, callKeyOf, rustEngineAvailable } from "./index.js";

/**
 * ADR 0051 D1 — `callKeyOf` and `callInputOf` are the engine's own canonical form, through napi:
 * the shared reference vectors (also checked by Rust and Python) hash to their call keys. They
 * take the arguments' JSON text, because a parsed JavaScript value has already lost what tells
 * `40` from `40.0` and rounds an integer above 2^53.
 */
type Vector = {
  description: string;
  name: string;
  inputJson: string;
  callInput: string;
  callKey: string;
};

const VECTORS = fileURLToPath(
  new URL("../../../crates/agents-core/tests/fixtures/call_key_vectors.json", import.meta.url)
);

const vectors = (): Vector[] =>
  (JSON.parse(readFileSync(VECTORS, "utf8")) as { vectors: Vector[] }).vectors;

const native = rustEngineAvailable() ? describe : describe.skip;

native("callKeyOf / callInputOf (ADR 0051 D1)", () => {
  it("every reference vector hashes to its call key", () => {
    expect(vectors().length).toBeGreaterThanOrEqual(20);
    for (const vector of vectors()) {
      expect({ description: vector.description, callInput: callInputOf(vector.inputJson) }).toEqual(
        { description: vector.description, callInput: vector.callInput }
      );
      expect({
        description: vector.description,
        callKey: callKeyOf(vector.name, vector.inputJson)
      }).toEqual({ description: vector.description, callKey: vector.callKey });
    }
  });

  it("a host checks a filed call with sha256 over callInput, and gets the key back from it", () => {
    for (const vector of vectors()) {
      const digest = createHash("sha256").update(vector.callInput, "utf8").digest("hex");
      expect(vector.callKey).toBe(`${vector.name}#${digest}`);
      expect(callKeyOf(vector.name, vector.callInput)).toBe(vector.callKey);
    }
  });

  it("refuses input that is not JSON text", () => {
    expect(() => callKeyOf("refund", "{")).toThrow();
    expect(() => callInputOf("not json")).toThrow();
  });
});
