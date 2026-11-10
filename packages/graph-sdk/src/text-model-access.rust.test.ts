import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { describe, expect, it } from "vitest";
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
const agent = (id: string) => ({
  id,
  type: "agent" as const,
  label: id,
  metadata: {
    agent: { provider: "mock", model: "mock", maxIterations: 1, outputChannel: "answer" }
  }
});
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

/** A refusal fails the run and names its code in an event; it is never written into the state. */
const expectRefused = (outcome: { status: string; state: unknown }, events: unknown[]): void => {
  expect(outcome.status).toBe("failed");
  expect(JSON.stringify(events)).toContain("text_model_access_denied");
  expect(JSON.stringify(outcome.state)).not.toContain("text_model_access_denied");
};

(rustEngineAvailable() ? describe : describe.skip)(
  "text-model policy across the real native seam",
  () => {
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
        const events: { type: string }[] = [];
        const outcome = await runCatalogGraph(graph(), {
          runId,
          initialData: {},
          tenantId: "tenant-a",
          textModelAccess: snapshot,
          streamTokens,
          onEvent: (event) => events.push(event)
        });
        expectRefused(outcome, events);
        expect(events.filter((event) => event.type === "token_delta")).toEqual([]);
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
                subAgent: { provider: "mock", model: "mock", maxIterations: 1 }
              }
            }
          }
        ],
        entryNodeId: "fan"
      } as unknown as GraphDefinition;
      const events: unknown[] = [];
      const outcome = await runCatalogGraph(fan, {
        runId,
        initialData: { items: ["one", "two"] },
        tenantId: "tenant-a",
        textModelAccess: snapshot,
        onEvent: (event) => events.push(event)
      });
      expectRefused(outcome, events);
      expect((outcome.state as unknown as { textModelAccess: unknown }).textModelAccess).toEqual(
        snapshot
      );
    });
    it("scopes a child graph and its agents to the parent snapshot", async () => {
      const child = graph();
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
      await expect(
        runCatalogGraph(parent, {
          runId,
          initialData: {},
          subgraphs: [child],
          tenantId: "tenant-a",
          textModelAccess: policy(runId, [])
        })
      ).rejects.toThrow("subgraph 'child' failed");
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
        const events: unknown[] = [];
        const outcome = await runCatalogGraph(graph(), {
          runId,
          initialData: {},
          tenantId: "tenant-a",
          textModelAccess: policy(runId, allowed),
          onEvent: (event) => events.push(event)
        });
        expectRefused(outcome, events);
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
          model: model.anthropic("claude-fixture"),
          prompt: { system: "offline boundary test" },
          maxIterations: 1
        })
        .compile({ tenantId: "tenant-a", textModelAccess: policy(runId, []) });
      const events: unknown[] = [];
      app.onEvent((event) => events.push(event));
      const state = await app.run({}, { runId });
      expectRefused({ status: state.status, state }, events);
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
