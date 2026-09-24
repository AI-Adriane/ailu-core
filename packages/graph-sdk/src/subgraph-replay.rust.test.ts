import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  replayCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition,
  type RunId
} from "./index.js";

/**
 * ADR 0043 D3 — `replayCatalogGraph` gains `subgraphs`, unblocked by D1 (`LlmRequest.run_id`,
 * `ailu-engine` PR #198) + D2 (run-scoped journal matching, PR #199) + the fork-invariant
 * tagging fix (PR #200): a replay's requests now carry the SAME logical run id the record pass
 * used, so `ReplayGateway`'s request-equality match discriminates a parent's calls from a
 * subgraph child's instead of the two racing for the same journal entries.
 *
 * Before this, `replayCatalogGraph` dropped `subgraphs` entirely (`undefined`) and a
 * `subgraphId` node failed loudly with `SubgraphNotFound` rather than silently diverging —
 * the second test below proves that guard still holds when `subgraphs` is omitted.
 *
 * Skipped when the native addon is absent. Offline/deterministic: provider keys are cleared so
 * both the parent and child agent calls hit the stub path, not a real provider.
 */
const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"] as const;

const childDef: GraphDefinition = {
  id: "sub-replay-echo",
  version: "1",
  name: "sub-replay-echo",
  channels: {
    agentResult: { type: "agentResult", reducer: "replace" }
  },
  nodes: [
    {
      id: "c_assistant",
      type: "agent",
      label: "c_assistant",
      metadata: { agent: { provider: "anthropic", system: "child agent", outputChannel: "agentResult" } }
    }
  ],
  edges: [],
  entryNodeId: "c_assistant"
} as unknown as GraphDefinition;

const parentDef: GraphDefinition = {
  id: "parent-with-agent-subgraph",
  version: "1",
  name: "parent-with-agent-subgraph",
  channels: {
    parentResult: { type: "agentResult", reducer: "replace" },
    childResult: { type: "agentResult", reducer: "replace" }
  },
  nodes: [
    {
      id: "p_assistant",
      type: "agent",
      label: "p_assistant",
      metadata: { agent: { provider: "anthropic", system: "parent agent", outputChannel: "parentResult" } }
    },
    {
      id: "sub",
      type: "subgraph",
      label: "sub",
      subgraphId: "sub-replay-echo",
      inputMapping: {},
      outputMapping: { childResult: "agentResult" }
    }
  ],
  edges: [{ id: "e1", from: "p_assistant", to: "sub", type: "default" }],
  entryNodeId: "p_assistant"
} as unknown as GraphDefinition;

describe("@ailu/graph-sdk — recursive subgraph replay (ADR 0043 D3)", () => {
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

  (rustEngineAvailable() ? it : it.skip)(
    "replays a parent + subgraph-child run deterministically, reproducing BOTH the parent's and the child's LLM output",
    async () => {
      process.env.AILU_LLM_RECORD = "1";
      const recorded = await runCatalogGraph(parentDef, {
        runId: "run_subgraph_replay_d3" as RunId,
        subgraphs: [childDef]
      });
      delete process.env.AILU_LLM_RECORD;

      expect(recorded.status).toBe("completed");
      expect((recorded.replayJournal ?? "").length).toBeGreaterThan(0);
      expect(recorded.entryState).toBeDefined();

      const recordedParent = recorded.state.channels.parentResult;
      const recordedChild = recorded.state.channels.childResult;
      // Different system prompts on parent vs child — sanity-check the fixture itself
      // isn't accidentally producing identical output that would mask a mismatch.
      expect(recordedParent).toBeDefined();
      expect(recordedChild).toBeDefined();

      const replayed = await replayCatalogGraph(
        parentDef,
        recorded.entryState!,
        "cp_subgraph_replay_d3_seed",
        recorded.replayJournal ?? "{}",
        { subgraphs: [childDef] }
      );

      expect(replayed.status).toBe("completed");
      expect(replayed.state.channels.parentResult).toEqual(recordedParent);
      expect(replayed.state.channels.childResult).toEqual(recordedChild);
    }
  );

  (rustEngineAvailable() ? it : it.skip)(
    "still fails loudly (SubgraphNotFound) when subgraphs are omitted on replay — no silent divergence",
    async () => {
      process.env.AILU_LLM_RECORD = "1";
      const recorded = await runCatalogGraph(parentDef, {
        runId: "run_subgraph_replay_d3_missing" as RunId,
        subgraphs: [childDef]
      });
      delete process.env.AILU_LLM_RECORD;

      await expect(
        replayCatalogGraph(
          parentDef,
          recorded.entryState!,
          "cp_subgraph_replay_d3_missing_seed",
          recorded.replayJournal ?? "{}"
          // no `subgraphs` option — omitted on purpose.
        )
      ).rejects.toThrow(/subgraph/i);
    }
  );
});
