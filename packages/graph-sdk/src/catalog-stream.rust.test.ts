import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  docQaReferenceDefinition,
  runCatalogGraph,
  rustEngineAvailable,
  type RunEvent,
  type RunId
} from "./index.js";

/**
 * Per-token streaming on the CATALOG run path (ADR 0060 B). `runCatalogGraph({ streamTokens: true })`
 * must surface an agent node's generation as `token_delta` RunEvents over `onEvent` — the same wiring
 * the in-process builder path (`CompiledGraph.stream`) already exercises, now reachable for a catalog
 * run (e.g. Governed Ask). The deterministic offline mock gateway (no provider keys) streams the reply
 * so the assertion is reproducible.
 */
const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"];
const QUESTION = "How does Ailu resume a run?";
const DOCUMENTS = "Ailu is a resumable agent graph runtime. It checkpoints after every node.";

describe("@ailu-ai/graph-sdk — catalog per-token streaming (ADR 0060 B)", () => {
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

  const runCollecting = async (streamTokens: boolean): Promise<RunEvent[]> => {
    const events: RunEvent[] = [];
    const outcome = await runCatalogGraph(docQaReferenceDefinition(), {
      runId: `run_catalog_stream_${streamTokens}` as RunId,
      initialData: { question: QUESTION, documents: DOCUMENTS },
      streamTokens,
      onEvent: (event) => events.push(event)
    });
    expect(outcome.status).toBe("completed");
    return events;
  };

  (rustEngineAvailable() ? it : it.skip)(
    "emits token_delta events when streamTokens is true",
    async () => {
      const deltas = (await runCollecting(true)).filter((e) => e.type === "token_delta");
      expect(deltas.length).toBeGreaterThanOrEqual(1);
      const first = deltas[0];
      if (first?.type === "token_delta") {
        expect(first.delta.length).toBeGreaterThan(0);
        expect(first.nodeId.length).toBeGreaterThan(0);
      }
    }
  );

  (rustEngineAvailable() ? it : it.skip)(
    "emits no token_delta when streamTokens is left off (default)",
    async () => {
      const deltas = (await runCollecting(false)).filter((e) => e.type === "token_delta");
      expect(deltas.length).toBe(0);
    }
  );
});
