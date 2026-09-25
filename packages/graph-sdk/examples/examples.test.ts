import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

// Aliased to source in vitest.config.ts, like the examples' own imports.
import { DEFAULT_KEY_ENV, rustEngineAvailable } from "@ailu-ai/graph-sdk";

/**
 * Runs every example in this folder, in-process, one test per file: the examples are
 * self-checking (a failed check throws), so importing one runs it end to end and a broken
 * example fails its test. An example that exports a `main` function (because it also exports
 * building blocks for other code) is run by calling it.
 *
 * The run is offline: vitest.config.ts sets AILU_LLM_MOCK=1 and this suite clears every
 * provider key, so agents answer from the engine's deterministic mock and never call a paid
 * API, even on a machine that has keys set.
 */
const EXAMPLES = readdirSync(fileURLToPath(new URL(".", import.meta.url)))
  .filter((file) => file.endsWith(".ts") && !file.endsWith(".test.ts"))
  .sort();

const PROVIDER_KEYS = [
  ...Object.values(DEFAULT_KEY_ENV).filter((name): name is string => name !== null),
  "GOOGLE_API_KEY",
  "HUGGINGFACE_API_KEY",
  "AILU_USE_OLLAMA"
];

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("examples (offline, self-checking)", () => {
  const saved = new Map<string, string | undefined>();

  beforeAll(() => {
    for (const key of PROVIDER_KEYS) {
      saved.set(key, process.env[key]);
      delete process.env[key];
    }
  });

  afterAll(() => {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });

  it("finds the examples", () => {
    expect(EXAMPLES.length).toBeGreaterThan(0);
  });

  for (const file of EXAMPLES) {
    it(`runs ${file}`, { timeout: 60_000 }, async () => {
      // Keep the examples' output out of the test log; replay it if the example fails.
      const output: string[] = [];
      const log = vi.spyOn(console, "log").mockImplementation((...args: unknown[]) => {
        output.push(args.map(String).join(" "));
      });
      const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk: string | Uint8Array) => {
        output.push(String(chunk));
        return true;
      });
      try {
        const example = (await import(`./${file}`)) as { main?: () => Promise<void> };
        if (typeof example.main === "function") {
          await example.main();
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        throw new Error(`${file} failed: ${message}\n--- its output ---\n${output.join("\n")}`, { cause: error });
      } finally {
        log.mockRestore();
        write.mockRestore();
      }
    });
  }
});
