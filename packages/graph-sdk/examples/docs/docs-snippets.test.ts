import { readdirSync } from "node:fs";

import { describe, it } from "vitest";

import { rustEngineAvailable } from "../../src/index.js";

/**
 * Every file in this folder is shown in the documentation (docs-site pages include them with
 * `file=...`). Importing one runs it: it must finish without throwing, offline (the test config
 * sets AILU_LLM_MOCK=1), so a page never shows code that does not work.
 */
const snippets = readdirSync(new URL(".", import.meta.url)).filter(
  (name) => name.endsWith(".ts") && !name.endsWith(".test.ts")
);

describe.skipIf(!rustEngineAvailable())("documentation snippets run", () => {
  for (const name of snippets) {
    it(name, async () => {
      await import(`./${name}`);
    });
  }
});
