import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  HostNodeBindingError,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition
} from "./index.js";

/**
 * ADR 0045 D3.2 — golden cases for the catalog spec. Each case is a catalog definition plus the
 * host's bindings; its `expected` is the `EngineSpec` the TypeScript SDK built for it before the
 * engine took this over (what `runCatalogGraph` handed the engine, without the per-call `runId` /
 * `initialData` / `inbox` / `streamTokens`), the warnings it printed, or the error it threw. The
 * file was recorded from that SDK (2.2.0) and is frozen: the engine's `spec_from_catalog` must keep
 * giving the same, through `runCatalogGraph` and through `engineSpecFromCatalog` — and through the
 * Rust tests of `crates/runtime-bridge` and the Python tests, which read the same file. A deliberate
 * change to the spec edits the file by hand.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../crates/runtime-bridge/tests/fixtures/catalog_spec_golden.json", import.meta.url)
);

type CatalogInput = {
  graph: unknown;
  subgraphs?: unknown[];
  hostNodes?: string[];
  hostTools?: string[];
  providerKeys?: Record<string, string>;
  fsPolicy?: unknown[];
  skills?: unknown[];
};

/** What the SDK refuses: a binding (with its node and reason), or a carrier or graph it threw on. */
type GoldenError =
  | { kind: "hostNodeBinding"; nodeId: string; reason: string }
  | { kind: "invalidCarrier"; nodeId: string }
  | { kind: "invalidDefinition" };

type GoldenExpected =
  { spec: Record<string, unknown>; warnings: string[] } | { error: GoldenError };

type GoldenCase = { name: string; input: CatalogInput; expected: GoldenExpected };

const PER_CALL_FIELDS = [
  "runId",
  "initialData",
  "inbox",
  "streamTokens",
  "state",
  "approvedTools",
  "replayJournal"
];

/** The spec the engine received, as `spec_from_catalog` returns it: no per-call fields, `hostNodeIds`. */
const staticSpec = (specJson: string): Record<string, unknown> => {
  const spec = JSON.parse(specJson) as Record<string, unknown>;
  for (const field of PER_CALL_FIELDS) delete spec[field];
  if ("jsNodeIds" in spec) {
    spec.hostNodeIds = spec.jsNodeIds;
    delete spec.jsNodeIds;
  }
  return spec;
};

const napi = (): {
  engineRun: (...args: unknown[]) => Promise<string>;
  engineSpecFromCatalog: (inputJson: string) => string;
} => createRequire(import.meta.url)("@ailu-ai/napi");

const completed = JSON.stringify({
  state: {
    runId: "run",
    graphId: "g",
    currentNodeId: "a",
    status: "completed",
    channels: {},
    version: 1,
    createdAt: "0",
    updatedAt: "0"
  },
  status: "completed",
  pendingApprovals: []
});

/**
 * What the SDK does with a case: run it through `runCatalogGraph` with the engine's `engineRun`
 * stubbed, and keep the spec it was handed, the warnings printed, or the error thrown.
 */
const sdkResult = async (input: CatalogInput, declared?: GoldenError): Promise<GoldenExpected> => {
  const warnings: string[] = [];
  let received: string | undefined;
  const warn = vi.spyOn(console, "warn").mockImplementation((message: unknown) => {
    warnings.push(String(message).replace(/^\[ailu\] /, ""));
  });
  const run = vi.spyOn(napi(), "engineRun").mockImplementation(async (specJson: unknown) => {
    received = specJson as string;
    return completed;
  });
  try {
    await runCatalogGraph(input.graph as GraphDefinition, {
      subgraphs: input.subgraphs as GraphDefinition[] | undefined,
      nodes: input.hostNodes?.map((id) => ({ id, execute: () => ({}) })),
      tools: input.hostTools?.map((name) => ({ name, execute: async () => ({}) })),
      providerKeys: input.providerKeys,
      fsPolicy: input.fsPolicy as never,
      skills: input.skills as never
    });
  } catch (error) {
    if (error instanceof HostNodeBindingError) {
      const reason = error.message.slice(`Host node '${error.nodeId}' can't be bound: `.length, -1);
      return { error: { kind: "hostNodeBinding", nodeId: error.nodeId, reason } };
    }
    if (error instanceof TypeError && declared !== undefined) {
      return { error: declared };
    }
    throw error;
  } finally {
    warn.mockRestore();
    run.mockRestore();
  }
  if (received === undefined) throw new Error("the engine was never called");
  return { spec: staticSpec(received), warnings };
};

/** What the engine's `spec_from_catalog` gives for a case, in the golden shape. */
const engineResult = (input: CatalogInput): GoldenExpected => {
  const out = JSON.parse(napi().engineSpecFromCatalog(JSON.stringify(input))) as
    | { spec: Record<string, unknown>; warnings: string[] }
    | { error: { kind: string; nodeId?: string; reason?: string } };
  if ("error" in out) {
    const { kind, nodeId, reason } = out.error;
    return {
      error: (kind === "invalidDefinition"
        ? { kind }
        : reason === undefined
          ? { kind, nodeId }
          : { kind, nodeId, reason }) as GoldenError
    };
  }
  return out;
};

const readGolden = (): GoldenCase[] => JSON.parse(readFileSync(FIXTURE, "utf8")) as GoldenCase[];

const native = rustEngineAvailable() ? describe : describe.skip;

native("@ailu-ai/graph-sdk — catalog spec golden cases (ADR 0045 D3.2)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("the SDK builds the golden spec for every case", async () => {
    expect(readGolden().length).toBeGreaterThanOrEqual(30);
    for (const golden of readGolden()) {
      const declared = "error" in golden.expected ? golden.expected.error : undefined;
      expect({ name: golden.name, ...(await sdkResult(golden.input, declared)) }).toEqual({
        name: golden.name,
        ...golden.expected
      });
    }
  });

  it("the engine's spec_from_catalog builds the golden spec for every case", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, ...engineResult(golden.input) }).toEqual({
        name: golden.name,
        ...golden.expected
      });
    }
  });
});
