import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { RunEvent, StreamEvent } from "@ailu-ai/graph-runtime";
import { DEFAULT_KEY_ENV, KEY_ENV_ALIASES } from "@ailu-ai/model-core";
import { describe, expect, it } from "vitest";

/**
 * The hand-written reference pages (errors, events, environment variables) must match the code.
 * Each test reads what the code defines and what the page documents, and fails on any difference
 * in either direction, naming what to add or remove.
 */

const REPO = fileURLToPath(new URL("../../../", import.meta.url));
const read = (path: string): string => readFileSync(join(REPO, path), "utf8");
const page = (name: string): string => read(`docs-site/docs/reference/${name}.md`);

const sorted = (values: Iterable<string>): string[] => [...new Set(values)].sort();
const difference = (a: Iterable<string>, b: Iterable<string>): string[] => {
  const exclude = new Set(b);
  return sorted([...a].filter((value) => !exclude.has(value)));
};

const filesUnder = (dir: string, extension: string, skip: (path: string) => boolean): string[] => {
  const out: string[] = [];
  const walk = (current: string): void => {
    for (const entry of readdirSync(current)) {
      const path = join(current, entry);
      if (entry === "node_modules" || entry === "dist" || skip(path)) continue;
      if (statSync(path).isDirectory()) walk(path);
      else if (path.endsWith(extension)) out.push(path);
    }
  };
  walk(dir);
  return out;
};

/** Non-test TypeScript sources of the workspace packages (those with a package.json). */
const tsSources = (): string[] =>
  readdirSync(join(REPO, "packages"))
    .map((name) => join(REPO, "packages", name))
    .filter((dir) => existsSync(join(dir, "package.json")) && existsSync(join(dir, "src")))
    .flatMap((dir) => filesUnder(join(dir, "src"), ".ts", (path) => /\.test\.ts$|\.d\.ts$/.test(path)));

/** Rust library sources, without their `#[cfg(test)]` modules. The control-plane config crate is out of scope. */
const rustSources = (): string[] =>
  readdirSync(join(REPO, "crates"))
    .filter((name) => name !== "config" && name !== "target")
    .map((name) => join(REPO, "crates", name, "src"))
    .filter((dir) => existsSync(dir))
    .flatMap((dir) => filesUnder(dir, ".rs", () => false));

const withoutComments = (source: string): string =>
  source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
const withoutRustTests = (source: string): string => source.split("#[cfg(test)]")[0] ?? source;

const matches = (source: string, pattern: RegExp): string[] =>
  [...source.matchAll(pattern)].map((match) => match[1] as string);

/** The backticked names in the first column of each table row. */
const firstColumnNames = (markdown: string): string[] =>
  markdown
    .split("\n")
    .filter((line) => line.startsWith("| `"))
    .flatMap((line) => matches(line.split("|")[1] ?? "", /`([^`]+)`/g));

/** The `AILU_` codes the SDK's errors carry (`code: "AILU_…"` or `code = "AILU_…"`). */
const thrownCodes = (): string[] =>
  sorted(
    tsSources().flatMap((file) =>
      matches(withoutComments(readFileSync(file, "utf8")), /\bcode\s*[:=]\s*"(AILU_[A-Z0-9_]+)"/g)
    )
  );

describe("reference: errors", () => {
  it("documents every AILU_ error code the SDK throws, and no other", () => {
    const thrown = thrownCodes();
    const documented = sorted(matches(page("errors"), /^### (AILU_[A-Z0-9_]+)$/gm));

    expect(thrown.length).toBeGreaterThan(5);
    expect({ undocumented: difference(thrown, documented), notInCode: difference(documented, thrown) }).toEqual({
      undocumented: [],
      notInCode: []
    });
  });
});

// The fields of each event, beyond `type` (and `runId`/`timestamp`, common to every run event).
// These maps are checked by the compiler: adding an event or a field to the types, or removing
// one, fails the typecheck until the map (and so the page) is updated.
type FieldsOf<U extends { type: string }, T extends U["type"], Common extends PropertyKey> = Exclude<
  keyof Extract<U, { type: T }>,
  "type" | Common
>;
const RUN_EVENT_FIELDS: {
  [T in RunEvent["type"]]: Record<FieldsOf<RunEvent, T, "runId" | "timestamp">, true>;
} = {
  node_started: { nodeId: true },
  node_completed: { nodeId: true, output: true },
  node_failed: { nodeId: true, error: true, attempt: true, category: true },
  node_error_routed: { nodeId: true, toNodeId: true, errorEdgeId: true, error: true, category: true },
  run_suspended: { nodeId: true, reason: true },
  run_resumed: { nodeId: true },
  run_completed: { finalState: true },
  run_failed: { error: true },
  run_cancelled: { nodeId: true },
  token_delta: { nodeId: true, messageId: true, delta: true, parentRunId: true, spawnId: true }
};
const STREAM_EVENT_FIELDS: { [T in StreamEvent["type"]]: Record<FieldsOf<StreamEvent, T, never>, true> } = {
  message_delta: { delta: true, nodeId: true, messageId: true },
  tool_call: { toolId: true, input: true, nodeId: true },
  state_update: { nodeId: true, delta: true },
  state_value: { state: true },
  debug: { nodeId: true, payload: true }
};

/** `{ type: [fields] }` from the page's table rows: the type column and the fields column. */
const eventTable = (section: string, typeColumn: number, fieldsColumn: number): Record<string, string[]> => {
  const table: Record<string, string[]> = {};
  const lines = section.split("\n");
  // Data rows only: a header row is the one followed by the `| --- |` separator.
  const rows = lines.filter((row, index) => row.startsWith("| ") && !lines[index + 1]?.startsWith("| ---"));
  for (const line of rows) {
    const cells = line.split("|");
    const type = matches(cells[typeColumn] ?? "", /`([a-z_]+)`/g)[0];
    if (type === undefined) continue;
    table[type] = sorted(matches(cells[fieldsColumn] ?? "", /`([A-Za-z]+)`/g));
  }
  return table;
};
const expected = (fields: Record<string, Record<string, true>>): Record<string, string[]> =>
  Object.fromEntries(Object.entries(fields).map(([type, map]) => [type, sorted(Object.keys(map))]));

describe("reference: events", () => {
  const [runSection = "", streamSection = ""] = page("events").split("## Stream events");

  it("lists every run event with its fields", () => {
    expect(eventTable(runSection, 1, 2)).toEqual(expected(RUN_EVENT_FIELDS));
  });

  it("lists every stream event with its fields", () => {
    expect(eventTable(streamSection, 2, 3)).toEqual(expected(STREAM_EVENT_FIELDS));
  });
});

// Read by the code but documented elsewhere on purpose.
const ENV_DOCUMENTED_ELSEWHERE = new Set([
  "AILU_SDK_ENGINE" // removed; see the migration page
]);

describe("reference: environment variables", () => {
  it("documents every variable the engine and the SDK read, and no other", () => {
    const rust = rustSources().flatMap((file) =>
      matches(withoutComments(withoutRustTests(readFileSync(file, "utf8"))), /"(AILU_[A-Z0-9_]+)"/g)
    );
    const typescript = tsSources().flatMap((file) => {
      const source = withoutComments(readFileSync(file, "utf8"));
      return [
        ...matches(source, /process\.env\.([A-Z][A-Z0-9_]+)/g),
        ...matches(source, /process\.env\[\s*"([A-Z][A-Z0-9_]+)"\s*\]/g),
        // Named constants, e.g. OFFLINE_MOCK_ENV = "AILU_LLM_MOCK".
        ...matches(source, /"(AILU_[A-Z0-9_]+)"/g)
      ];
    });
    const errorCodes = new Set(thrownCodes());
    const providerKeys = [
      ...Object.values(DEFAULT_KEY_ENV).filter((name): name is string => name !== null),
      ...Object.values(KEY_ENV_ALIASES).flatMap((names) => [...(names ?? [])])
    ];
    const read = new Set(
      [...rust, ...typescript, ...providerKeys].filter(
        (name) => !errorCodes.has(name) && !ENV_DOCUMENTED_ELSEWHERE.has(name)
      )
    );
    const documented = firstColumnNames(page("environment"))
      .map((name) => name.replace(/=.*$/, ""))
      .filter((name) => /^[A-Z][A-Z0-9_]+$/.test(name));

    expect(read.size).toBeGreaterThan(10);
    expect({ undocumented: difference(read, documented), notInCode: difference(documented, read) }).toEqual({
      undocumented: [],
      notInCode: []
    });
  });
});
