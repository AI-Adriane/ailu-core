import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  createGraph,
  DefaultLLMGateway,
  model,
  runCatalogGraph,
  resumeCatalogGraph,
  rustEngineAvailable,
  textModelAccessVersion,
  type GraphDefinition,
  type RunId,
  type TextModelAccessSnapshot
} from "./index.js";

const policy = (
  runId: string,
  allowedModels = [{ provider: "mock", modelId: "mock" }]
): TextModelAccessSnapshot => {
  const enabledModels = [...allowedModels].sort((a, b) =>
    a.provider < b.provider
      ? -1
      : a.provider > b.provider
        ? 1
        : a.modelId < b.modelId
          ? -1
          : a.modelId > b.modelId
            ? 1
            : 0
  );
  const definition = { schemaVersion: 1, catalogRevision: "catalog-test", enabledModels };
  return {
    schemaVersion: 1,
    tenantId: "tenant-a",
    runId,
    policyId: "organization-text-models",
    policyVersion: 1,
    catalogRevision: "catalog-test",
    policyHash: createHash("sha256").update(JSON.stringify(definition)).digest("hex"),
    allowedModels
  };
};
const agent = (id: string, provider = "mock", model = "mock") => ({
  id,
  type: "agent" as const,
  label: id,
  metadata: { agent: { provider, model, maxIterations: 1, outputChannel: "answer" } }
});

/**
 * The provider of every refused run: a local server that counts the requests it receives. The
 * engine reaches it as Ollama, whose address the operator sets (not the graph), so the allowlist
 * is the only thing between an agent and this server.
 */
const COUNTED = { provider: "ollama", modelId: "counted" };
let providerCalls = 0;
const countingProvider = createServer((request, response) => {
  providerCalls += 1;
  request.resume();
  response.writeHead(200, { "content-type": "application/json" });
  response.end(
    JSON.stringify({
      choices: [
        { index: 0, message: { role: "assistant", content: "FINAL: done" }, finish_reason: "stop" }
      ],
      usage: { prompt_tokens: 1, completion_tokens: 1 }
    })
  );
});
const countedAgent = (id: string) => agent(id, COUNTED.provider, COUNTED.modelId);
const countedGraph = (): GraphDefinition =>
  ({ ...graph(), nodes: [countedAgent("a")] }) as unknown as GraphDefinition;

const graph = (): GraphDefinition =>
  ({
    id: "model-policy",
    version: "1",
    name: "policy",
    channels: { answer: { type: "string", reducer: "replace", default: "" } },
    nodes: [agent("a")],
    edges: [],
    entryNodeId: "a"
  }) as unknown as GraphDefinition;

const DENIED = "text_model_access_denied";
type SeenEvent = { type: string; runId?: string; nodeId?: string; error?: string };

/**
 * A refusal, as the engine exposes it: the node fails with exactly the denial code and so does
 * its run, no node completes, and the provider is never called. `failed` names each run that
 * must fail and the node it fails at.
 */
const expectRefusedEvents = (events: SeenEvent[], failed: [runId: string, nodeId: string][]) => {
  for (const [runId, nodeId] of failed) {
    const ofRun = events.filter((event) => event.runId === runId);
    expect(ofRun.filter((event) => event.type === "node_failed")).toEqual([
      expect.objectContaining({ nodeId, error: DENIED })
    ]);
    expect(ofRun.filter((event) => event.type === "run_failed")).toEqual([
      expect.objectContaining({ error: DENIED })
    ]);
  }
  // Every failure of the run, a fan-out spawn included, is this refusal and nothing else.
  for (const event of events.filter((event) => event.type.endsWith("_failed"))) {
    expect(event.error).toBe(DENIED);
  }
  expect(events.filter((event) => event.type === "node_completed")).toEqual([]);
  expect(events.filter((event) => event.type === "token_delta")).toEqual([]);
  expect(providerCalls).toBe(0);
};
/** The same refusal seen from the outcome: failed, and nothing of it written into the state. */
const expectRefused = (
  outcome: { status: string; state: unknown },
  events: SeenEvent[],
  nodeId: string
): void => {
  const state = outcome.state as { runId: string; status: string; channels: object };
  expect(outcome.status).toBe("failed");
  expect(state.status).toBe("failed");
  expectRefusedEvents(events, [[state.runId, nodeId]]);
  expect(JSON.stringify(state)).not.toContain(DENIED);
  expect(JSON.stringify(state.channels)).not.toContain("error");
};

(rustEngineAvailable() ? describe : describe.skip)(
  "text-model policy across the real native seam",
  () => {
    const saved = { use: process.env.AILU_USE_OLLAMA, url: process.env.AILU_OLLAMA_BASE_URL };
    beforeAll(async () => {
      await new Promise<void>((resolve) => countingProvider.listen(0, "127.0.0.1", resolve));
      const { port } = countingProvider.address() as AddressInfo;
      process.env.AILU_USE_OLLAMA = "1";
      process.env.AILU_OLLAMA_BASE_URL = `http://127.0.0.1:${port}/v1`;
    });
    afterAll(async () => {
      for (const [name, value] of [
        ["AILU_USE_OLLAMA", saved.use],
        ["AILU_OLLAMA_BASE_URL", saved.url]
      ] as const) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
      await new Promise((resolve) => countingProvider.close(resolve));
    });
    beforeEach(() => {
      providerCalls = 0;
    });
    it("the counting provider is called when its model is allowed", async () => {
      const runId = "counted-allowed" as RunId;
      const outcome = await runCatalogGraph(countedGraph(), {
        runId,
        initialData: {},
        tenantId: "tenant-a",
        textModelAccess: policy(runId, [COUNTED])
      });
      expect(outcome.status).toBe("completed");
      expect(providerCalls).toBe(1);
    });
    it("reports capability from the actually loaded addon", () => {
      expect(textModelAccessVersion()).toBe(1);
    });
    it("the loaded addon explicitly supports schema 1", () => {
      const native = createRequire(import.meta.url)("@ailu-ai/napi") as {
        textModelAccessVersion?: () => number;
      };
      expect(native.textModelAccessVersion?.()).toBe(1);
    });
    it.each([false, true])(
      "refuses a model without provider output (stream=%s), preserving its binding",
      async (streamTokens) => {
        const runId = `denied-${streamTokens}` as RunId;
        const snapshot = policy(runId, []);
        const events: SeenEvent[] = [];
        const outcome = await runCatalogGraph(countedGraph(), {
          runId,
          initialData: {},
          tenantId: "tenant-a",
          textModelAccess: snapshot,
          streamTokens,
          onEvent: (event) => events.push(event)
        });
        expectRefused(outcome, events, "a");
        expect((outcome.state as unknown as { textModelAccess: unknown }).textModelAccess).toEqual(
          snapshot
        );
      }
    );
    it("does not widen access for concurrent mapAgents spawns", async () => {
      const runId = "fan-policy" as RunId;
      const snapshot = policy(runId, []);
      const fan = {
        ...graph(),
        channels: {
          items: { type: "json", reducer: "replace", default: [] },
          summaries: { type: "json", reducer: "replace", default: [] }
        },
        nodes: [
          {
            id: "fan",
            type: "action",
            label: "fan",
            metadata: {
              mapAgents: {
                overChannel: "items",
                joinAt: "summaries",
                subAgent: { provider: COUNTED.provider, model: COUNTED.modelId, maxIterations: 1 }
              }
            }
          }
        ],
        entryNodeId: "fan"
      } as unknown as GraphDefinition;
      const events: SeenEvent[] = [];
      const outcome = await runCatalogGraph(fan, {
        runId,
        initialData: { items: ["one", "two"] },
        tenantId: "tenant-a",
        textModelAccess: snapshot,
        onEvent: (event) => events.push(event)
      });
      expectRefused(outcome, events, "fan");
      expect(events.filter((event) => event.type === "spawn_failed")).toHaveLength(2);
      expect(outcome.state.channels).toEqual({ items: ["one", "two"], summaries: [] });
      expect((outcome.state as unknown as { textModelAccess: unknown }).textModelAccess).toEqual(
        snapshot
      );
    });
    it("scopes a child graph and its agents to the parent snapshot", async () => {
      const child = countedGraph();
      child.id = "child" as GraphDefinition["id"];
      const parent: GraphDefinition = {
        ...graph(),
        nodes: [
          {
            id: "sub",
            type: "subgraph",
            label: "sub",
            subgraphId: "child",
            outputMapping: { answer: "answer" }
          }
        ],
        entryNodeId: "sub"
      } as unknown as GraphDefinition;
      const runId = "parent-policy" as RunId;
      const events: SeenEvent[] = [];
      // The engine reports a failed child as a runtime error of the parent, without an outcome:
      // the cause is read where it is exposed, in the child's own events.
      await expect(
        runCatalogGraph(parent, {
          runId,
          initialData: {},
          subgraphs: [child],
          tenantId: "tenant-a",
          textModelAccess: policy(runId, []),
          onEvent: (event) => events.push(event)
        })
      ).rejects.toThrow("subgraph 'child' failed");
      expectRefusedEvents(events, [[`${runId}:sub`, "a"]]);
    });
    it("suspends and resumes with the original policy, rejecting omission and replacement", async () => {
      const gated: GraphDefinition = {
        ...graph(),
        nodes: [{ id: "gate", type: "human-gate", label: "gate" }, agent("a")],
        edges: [{ id: "e", from: "gate", to: "a", type: "default" }],
        entryNodeId: "gate"
      } as unknown as GraphDefinition;
      const runId = "resume-policy" as RunId;
      const snapshot = policy(runId);
      const suspended = await runCatalogGraph(gated, {
        runId,
        initialData: {},
        tenantId: "tenant-a",
        textModelAccess: snapshot
      });
      expect(suspended.status).toBe("suspended");
      await expect(resumeCatalogGraph(gated, suspended.state)).rejects.toThrow(
        "text_model_access_snapshot_mismatch"
      );
      await expect(
        resumeCatalogGraph(gated, suspended.state, {
          tenantId: "tenant-a",
          textModelAccess: policy(runId, [])
        })
      ).rejects.toThrow("text_model_access_snapshot_mismatch");
      const resumed = await resumeCatalogGraph(gated, suspended.state, {
        tenantId: "tenant-a",
        textModelAccess: snapshot
      });
      expect(resumed.status).toBe("completed");
      expect(JSON.stringify(resumed.state)).not.toContain("text_model_access_denied");
    });
    it("matches the host canonical hash for Unicode UTF-16 order and reversed input", async () => {
      const models = [
        { provider: "mock", modelId: "\ue000" },
        { provider: "mock", modelId: "😀" }
      ];
      for (const allowed of [models, [...models].reverse()]) {
        const runId = "unicode-policy" as RunId;
        const events: SeenEvent[] = [];
        const outcome = await runCatalogGraph(countedGraph(), {
          runId,
          initialData: {},
          tenantId: "tenant-a",
          textModelAccess: policy(runId, allowed),
          onEvent: (event) => events.push(event)
        });
        expectRefused(outcome, events, "a");
      }
    });
    it("enforces the same scoped policy on standalone native model calls", async () => {
      const native = createRequire(import.meta.url)("@ailu-ai/napi") as {
        llmComplete(request: string, keys: string): Promise<string>;
      };
      const base = {
        provider: "mock",
        model: "mock",
        messages: [],
        tenantId: "tenant-a",
        runId: "standalone-policy"
      };
      await expect(
        native.llmComplete(
          JSON.stringify({ ...base, textModelAccess: policy(base.runId, []) }),
          "{}"
        )
      ).rejects.toThrow("text_model_access_denied");
      await expect(
        native.llmComplete(
          JSON.stringify({ ...base, tenantId: "tenant-b", textModelAccess: policy(base.runId) }),
          "{}"
        )
      ).rejects.toThrow("text_model_access_invalid");
      const allowed = JSON.parse(
        await native.llmComplete(
          JSON.stringify({ ...base, textModelAccess: policy(base.runId) }),
          "{}"
        )
      ) as { content: string };
      expect(allowed.content.length).toBeGreaterThan(0);
    });
    it("honors the same policy on the builder's native agent path", async () => {
      const runId = "builder-policy" as RunId;
      const app = createGraph({ name: "governed-builder" })
        .agentNode("assistant", {
          llm: new DefaultLLMGateway(),
          model: model.ollama(COUNTED.modelId),
          prompt: { system: "offline boundary test" },
          maxIterations: 1
        })
        .compile({ tenantId: "tenant-a", textModelAccess: policy(runId, []) });
      const events: SeenEvent[] = [];
      app.onEvent((event) => events.push(event));
      const state = await app.run({}, { runId });
      expectRefused({ status: state.status, state }, events, "assistant");
    });
    it("preserves a legacy catalog call without inserting policy metadata", async () => {
      const outcome = await runCatalogGraph(graph(), {
        runId: "legacy-policy" as RunId,
        initialData: {}
      });
      expect(outcome.status).toBe("completed");
      expect(Object.hasOwn(outcome.state, "textModelAccess")).toBe(false);
    });
  }
);
