import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  createGraph,
  DefaultLLMGateway,
  InvalidFanOutLimitsError,
  replayCatalogGraph,
  resumeCatalogGraph,
  runCatalogGraph,
  type FanOutLimits,
  type GraphDefinition
} from "./index.js";

const child = {
  id: "child",
  version: "1",
  name: "child",
  channels: {},
  nodes: [{ id: "work", type: "action", label: "work" }],
  edges: [],
  entryNodeId: "work"
} as unknown as GraphDefinition;
const parent = {
  id: "parent",
  version: "1",
  name: "parent",
  channels: {
    items: { type: "json", reducer: "replace" },
    results: { type: "json", reducer: "replace" }
  },
  nodes: [
    {
      id: "each",
      type: "subgraph",
      label: "each",
      subgraphId: "child",
      inputMapping: {},
      mapSubgraph: { overChannel: "items", joinAt: "results" }
    }
  ],
  edges: [],
  entryNodeId: "each"
} as unknown as GraphDefinition;

describe("host fan-out limits through the native SDK", () => {
  let mock: string | undefined;
  let record: string | undefined;
  beforeEach(() => {
    mock = process.env.AILU_LLM_MOCK;
    record = process.env.AILU_LLM_RECORD;
    process.env.AILU_LLM_MOCK = "1";
    delete process.env.AILU_LLM_RECORD;
  });
  afterEach(() => {
    if (mock === undefined) delete process.env.AILU_LLM_MOCK;
    else process.env.AILU_LLM_MOCK = mock;
    if (record === undefined) delete process.env.AILU_LLM_RECORD;
    else process.env.AILU_LLM_RECORD = record;
  });

  it("accepts 128 by default, refuses 129, and ignores graph/channel overrides", async () => {
    const forged = { maxItems: 1024, maxConcurrency: 32, maxTotalItems: 4096 };
    for (const count of [128, 129]) {
      let calls = 0;
      const graph = { ...parent, fanOutLimits: forged, metadata: { fanOutLimits: forged } };
      const done = await runCatalogGraph(graph, {
        subgraphs: [child],
        initialData: { items: Array(count).fill(null), fanOutLimits: forged },
        nodes: [
          {
            id: "work",
            execute: () => {
              calls++;
              return {};
            }
          }
        ]
      });
      expect(done.status).toBe(count === 128 ? "completed" : "failed");
      expect(calls).toBe(count === 128 ? 128 : 0);
    }
  });

  it("uses snapshotted builder host options, excluding them from the graph", async () => {
    const limits: FanOutLimits = { maxItems: 200, maxConcurrency: 2, maxTotalItems: 600 };
    const app = createGraph({ name: "host-map" })
      .channel("items", { type: "json", default: [] as string[] })
      .mapAgents("each", {
        overChannel: "items",
        joinAt: "results",
        subAgent: { llm: new DefaultLLMGateway(), prompt: { system: "Brief." }, maxIterations: 1 }
      })
      .compile({ fanOutLimits: limits });
    limits.maxItems = 1;
    expect("fanOutLimits" in app.definition).toBe(false);
    const done = await app.run({ items: Array(129).fill("x") as string[] });
    expect(done.status).toBe("completed");
    expect(done.channels.results).toHaveLength(129);
    for (const result of done.channels.results as { reasoning?: unknown; error?: unknown }[]) {
      expect(typeof result.reasoning).toBe("string");
      expect(result.error).toBeUndefined();
    }
  });

  it.each([
    { maxItems: 1025 },
    { maxTotalItems: 4097 },
    { maxConcurrency: 33 },
    { maxConcurrency: 0 }
  ])("rejects invalid options at construction with a typed error: %j", async (limits) => {
    expect(() =>
      createGraph({ name: "invalid" })
        .node("work", async () => ({}))
        .compile({ fanOutLimits: limits })
    ).toThrow(InvalidFanOutLimitsError);
    const checked = createGraph({ name: "invalid" })
      .node("work", async () => ({}))
      .safeCompile({ fanOutLimits: limits });
    expect(checked.success).toBe(false);
    if (!checked.success) expect(checked.error).toBeInstanceOf(InvalidFanOutLimitsError);
    await expect(
      runCatalogGraph(parent, { subgraphs: [child], fanOutLimits: limits })
    ).rejects.toBeInstanceOf(InvalidFanOutLimitsError);
  });

  it("forwards limits through catalog resume and replay", async () => {
    const limits = { maxItems: 200 };
    const gated = {
      ...parent,
      nodes: [{ id: "gate", type: "human-gate", label: "gate" }, ...parent.nodes],
      edges: [{ id: "go", from: "gate", to: "each", type: "default" }],
      entryNodeId: "gate"
    } as unknown as GraphDefinition;
    const options = { subgraphs: [child], fanOutLimits: limits };
    const waiting = await runCatalogGraph(gated, {
      ...options,
      initialData: { items: Array(129).fill(null) }
    });
    expect(waiting.status).toBe("suspended");
    expect((await resumeCatalogGraph(gated, waiting.state, options)).status).toBe("completed");

    process.env.AILU_LLM_RECORD = "1";
    const recorded = await runCatalogGraph(parent, {
      ...options,
      initialData: { items: Array(129).fill(null) }
    });
    delete process.env.AILU_LLM_RECORD;
    expect(recorded.entryState).toBeDefined();
    expect(recorded.replayJournal).toBeDefined();
    const replayed = await replayCatalogGraph(
      parent,
      recorded.entryState!,
      "entry",
      recorded.replayJournal!,
      options
    );
    expect(replayed.status).toBe("completed");
  });
});
