import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { API_KEY_ENV_ALLOWLIST_ENV, ApiKeyEnvNotAllowedError } from "./api-key-env.js";
import { MissingProviderKeyError, resolveProviderKeys } from "./index.js";

/**
 * The case table the engine (`crates/runtime-bridge/src/api_key_env.rs`) and the Python SDK run
 * too: the same `apiKeyEnv` and allow-list give the same key, refusal or missing key in all three.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../crates/runtime-bridge/tests/fixtures/api_key_env_cases.json", import.meta.url)
);

type Expectation = { key: string } | { keyless: true } | { refused: string } | { notSet: string };

type Case = {
  name: string;
  allowlist: string | null;
  apiKeyEnv: string | null;
  env: Record<string, string>;
  expect: Expectation;
};

const table = JSON.parse(readFileSync(FIXTURE, "utf8")) as { cases: Case[] };

/** What `resolveProviderKeys` does with a case, in the table's terms. */
const outcome = (testCase: Case): Expectation => {
  const env: Record<string, string | undefined> = { ...testCase.env };
  if (testCase.allowlist !== null) env[API_KEY_ENV_ALLOWLIST_ENV] = testCase.allowlist;
  try {
    const resolved = resolveProviderKeys(
      { baseURL: "http://endpoint.test/v1", apiKeyEnv: testCase.apiKeyEnv ?? undefined },
      env
    );
    const key = resolved.providerKeys[resolved.provider];
    return key === undefined ? { keyless: true } : { key };
  } catch (error) {
    if (error instanceof ApiKeyEnvNotAllowedError) return { refused: error.envVar };
    if (error instanceof MissingProviderKeyError) return { notSet: error.envVar };
    throw error;
  }
};

describe("the shared apiKeyEnv case table", () => {
  it("has cases", () => {
    expect(table.cases.length).toBeGreaterThan(0);
  });

  it.each(table.cases.map((testCase) => [testCase.name, testCase] as const))(
    "%s",
    (_name, testCase) => {
      expect(outcome(testCase)).toEqual(testCase.expect);
    }
  );
});
