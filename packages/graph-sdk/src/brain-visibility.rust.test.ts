import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { runCatalogGraph, rustEngineAvailable, type GraphDefinition, type RunId } from "./index.js";

/**
 * ADR 0047 — the brain is shown to an agent that may see it. `visibleChannels` bounds the
 * organisation's brain (`__brainRecall`) like the rest of the state: an agent narrowed to other
 * channels gets no « Governed knowledge » block. The recorded request (record mode) is what the
 * provider would have received.
 */
const PROVIDER_KEYS = [
  "MISTRAL_API_KEY",
  "ANTHROPIC_API_KEY",
  "OPENAI_API_KEY",
  "AILU_USE_OLLAMA"
] as const;

const graphWith = (visibleChannels: string[] | undefined): GraphDefinition =>
  ({
    id: "brain-visibility",
    version: "1",
    name: "brain-visibility",
    channels: {
      name: { type: "string", reducer: "replace" },
      answer: { type: "agentResult", reducer: "replace" }
    },
    nodes: [
      {
        id: "assistant",
        type: "agent",
        label: "assistant",
        metadata: {
          agent: {
            provider: "mock",
            system: "Answer.",
            outputChannel: "answer",
            ...(visibleChannels !== undefined ? { visibleChannels } : {})
          }
        }
      }
    ],
    edges: [],
    entryNodeId: "assistant"
  }) as unknown as GraphDefinition;

type Journal = { decisions?: { calls?: { request?: { messages?: { content?: unknown }[] } }[] } };

describe("@ailu-ai/graph-sdk — the brain follows visibleChannels (ADR 0047)", () => {
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    for (const key of [...PROVIDER_KEYS, "AILU_LLM_RECORD"]) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(() => {
    for (const key of [...PROVIDER_KEYS, "AILU_LLM_RECORD"]) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  });

  const firstMessage = async (visibleChannels: string[] | undefined): Promise<string> => {
    process.env.AILU_LLM_RECORD = "1";
    const outcome = await runCatalogGraph(graphWith(visibleChannels), {
      runId: `run_brain_${visibleChannels?.join("_") ?? "all"}` as RunId,
      initialData: { name: "Ada", __brainRecall: ["Acme — a customer since 2019"] } as never
    });
    delete process.env.AILU_LLM_RECORD;
    expect(outcome.status).toBe("completed");
    const journal = JSON.parse(outcome.replayJournal ?? "{}") as Journal;
    const content = journal.decisions?.calls?.[0]?.request?.messages?.[0]?.content;
    return typeof content === "string" ? content : "";
  };

  (rustEngineAvailable() ? it : it.skip)(
    "shows the brain to an agent that may see it, and not to one narrowed to other channels",
    async () => {
      expect(await firstMessage(undefined)).toContain("Governed knowledge");
      expect(await firstMessage(["name", "__brainRecall"])).toContain(
        "Acme — a customer since 2019"
      );

      const narrowed = await firstMessage(["name"]);
      expect(narrowed).not.toContain("Governed knowledge");
      expect(narrowed).not.toContain("Acme");
    }
  );
});
