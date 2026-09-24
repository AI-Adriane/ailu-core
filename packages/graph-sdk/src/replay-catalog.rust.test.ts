import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  docQaReferenceDefinition,
  replayCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  verifyReplayDecisions,
  type CatalogRunOutcome,
  type RunId
} from "./index.js";

/**
 * Replay-as-evidence (ADR 0038) — the napi PLUMBING proven end-to-end on the Rust engine.
 *
 * Records a governed catalog run (`AILU_LLM_RECORD`) so the engine journals its LLM outputs +
 * timestamp sequence, then drives {@link replayCatalogGraph} through `engineReplay` and asserts the
 * deterministic fork comes back without re-sampling. This guards the feature-detected replay path
 * from silently rotting; it does NOT assert full faithfulness from the run's *initial* checkpoint —
 * that needs the control plane's persisted checkpoint history + attestation chain and is L5's e2e.
 *
 * Skipped when the native addon is absent. Tolerant of an addon predating replay support.
 */

const QUESTION = "How does Ailu resume a run after a crash or an approval?";
const DOCUMENTS = "Ailu is a resumable agent graph runtime. It checkpoints after every node.";
const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"] as const;

const lacksReplaySupport = (error: unknown): boolean =>
  error instanceof Error && /no replay support|engineReplay/i.test(error.message);

describe("@ailu/graph-sdk — replay-as-evidence napi round-trip (ADR 0038)", () => {
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
    "records a run's journal, then replays it deterministically through engineReplay",
    async () => {
      const definition = docQaReferenceDefinition();

      // 1. RECORD: the engine journals each agent's LLM I/O + the run's timestamp sequence.
      process.env.AILU_LLM_RECORD = "1";
      const recorded = await runCatalogGraph(definition, {
        runId: "run_replay_evidence" as RunId,
        initialData: { question: QUESTION, documents: DOCUMENTS }
      });
      delete process.env.AILU_LLM_RECORD;

      expect(recorded.status).toBe("completed");
      expect(recorded.usedRustEngine).toBe(true);
      // Record mode surfaced a non-trivial journal (`{ decisions, clock }`) for the control plane.
      expect(typeof recorded.replayJournal).toBe("string");
      expect((recorded.replayJournal ?? "").length).toBeGreaterThan(0);

      // 2. REPLAY: re-feed the journal through the napi `engineReplay` bridge.
      let replayed: CatalogRunOutcome;
      try {
        replayed = await replayCatalogGraph(
          definition,
          recorded.state,
          "cp_replay_evidence_seed",
          recorded.replayJournal ?? "{}"
        );
      } catch (error) {
        if (lacksReplaySupport(error)) return; // addon predates replay — plumbing not present, skip.
        throw error;
      }

      expect(replayed.usedRustEngine).toBe(true);
      expect(replayed.status).toBe("completed");
      // `replay_from` forks a NEW deterministic run id (`<run>:fork:<seq>`) — the original is untouched.
      expect(String(replayed.state.runId).startsWith("run_replay_evidence:fork:")).toBe(true);
    }
  );

  (rustEngineAvailable() ? it : it.skip)(
    "surfaces the entry state in record mode, then re-derives the WHOLE run from it (ADR 0040)",
    async () => {
      const definition = docQaReferenceDefinition();

      // RECORD: a record-mode run surfaces its ENTRY state (the seed for replay_from) + the journal.
      process.env.AILU_LLM_RECORD = "1";
      const recorded = await runCatalogGraph(definition, {
        runId: "run_entry_state_evidence" as RunId,
        initialData: { question: QUESTION, documents: DOCUMENTS }
      });
      delete process.env.AILU_LLM_RECORD;

      expect(recorded.status).toBe("completed");
      // The entry state is present (record mode) and is the un-run initial state (version 0).
      expect(recorded.entryState).toBeDefined();
      const entryState = recorded.entryState as NonNullable<typeof recorded.entryState>;
      expect(entryState.version).toBe(0);
      expect((entryState as { status: string }).status).toBe("running");
      const recordedAnswer = recorded.state.channels.answer;

      // REPLAY from the ENTRY state (not the terminal one): re-derive the whole run deterministically.
      let replayed: CatalogRunOutcome;
      try {
        replayed = await replayCatalogGraph(
          definition,
          entryState,
          "cp_entry_state_seed",
          recorded.replayJournal ?? "{}"
        );
      } catch (error) {
        if (lacksReplaySupport(error)) return;
        throw error;
      }

      // The replay re-derived the full pipeline to completion — the SAME answer, from the start.
      expect(replayed.usedRustEngine).toBe(true);
      expect(replayed.status).toBe("completed");
      expect(replayed.state.channels.answer).toEqual(recordedAnswer);
      expect(String(replayed.state.runId).startsWith("run_entry_state_evidence:fork:")).toBe(true);
    }
  );

  it("verifyReplayDecisions confirms an attested chain reproduced by a replay", () => {
    // The faithfulness comparator over the two decision sets the control plane will derive (L5).
    const attested = [{ status: "approved", subject: "ship the release notes" }];
    const replayed = [{ status: "approved", subject: "ship the release notes" }];
    expect(verifyReplayDecisions(attested, replayed).ok).toBe(true);
    expect(verifyReplayDecisions(attested, [{ status: "rejected", subject: "ship the release notes" }]).ok).toBe(
      false
    );
  });
});
