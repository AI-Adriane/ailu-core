import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { runCatalogGraph, rustEngineAvailable, type GraphDefinition, type RunId } from "./index.js";

/**
 * ADR 0041 D1 — host tools on the CATALOG run path. `RunCatalogGraphOptions.tools` binds JS
 * `{ name, execute }` closures into the same napi host-tool seam the builder path uses: a catalog
 * agent whose `toolNames` includes a bound name gets the REAL tool; an unbound name keeps the no-op
 * stub (no behaviour change for existing graphs). The offline mock gateway (no provider keys) calls
 * every declared tool once then finishes — so the dispatch is deterministically observable.
 */
const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"];

const agentGraph = (toolNames: string[]): GraphDefinition =>
  ({
    id: "host-tools-e2e",
    version: "1",
    name: "host-tools-e2e",
    channels: {
      answer: { type: "agentResult", reducer: "replace" }
    },
    nodes: [
      {
        id: "worker",
        type: "agent",
        label: "worker",
        metadata: {
          agent: {
            provider: "mock",
            system: "Use your tools, then answer.",
            toolNames,
            outputChannel: "answer",
            approvalToolNames: [],
            suspendForApproval: false
          }
        }
      }
    ],
    edges: [],
    entryNodeId: "worker"
  }) as unknown as GraphDefinition;

describe("@ailu/graph-sdk — catalog host tools (ADR 0041 D1)", () => {
  const saved: Record<string, string | undefined> = {};
  beforeEach(() => {
    for (const key of PROVIDER_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });
  afterEach(() => {
    for (const key of PROVIDER_KEYS) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  });

  (rustEngineAvailable() ? it : it.skip)(
    "dispatches a bound tool name to the JS execute over the napi seam",
    async () => {
      const calls: unknown[] = [];
      const outcome = await runCatalogGraph(agentGraph(["lookup"]), {
        runId: "run_host_tool_bound" as RunId,
        initialData: {},
        tools: [
          {
            name: "lookup",
            execute: async (input: unknown) => {
              calls.push(input);
              return { hits: ["doc-1"] };
            }
          }
        ]
      });
      expect(outcome.status).toBe("completed");
      expect(calls.length).toBeGreaterThanOrEqual(1);
    }
  );

  (rustEngineAvailable() ? it : it.skip)(
    "leaves an UNBOUND tool name a no-op stub (no behaviour change), and never calls other bindings",
    async () => {
      const calls: unknown[] = [];
      const outcome = await runCatalogGraph(agentGraph(["unbound"]), {
        runId: "run_host_tool_unbound" as RunId,
        initialData: {},
        tools: [
          {
            name: "lookup",
            execute: async (input: unknown) => {
              calls.push(input);
              return {};
            }
          }
        ]
      });
      expect(outcome.status).toBe("completed");
      expect(calls).toHaveLength(0);
    }
  );

  (rustEngineAvailable() ? it : it.skip)(
    "runs exactly as before when no tools are supplied (stub path)",
    async () => {
      const outcome = await runCatalogGraph(agentGraph(["lookup"]), {
        runId: "run_host_tool_none" as RunId,
        initialData: {}
      });
      expect(outcome.status).toBe("completed");
    }
  );
});
