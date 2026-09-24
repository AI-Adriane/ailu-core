import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  buildDocQaReference,
  docQaReferenceDefinition,
  isCatalogGraph,
  readAgentCarrier,
  readComponentCarrier,
  runCatalogGraph,
  rustEngineAvailable,
  type RunId
} from "./index.js";

/**
 * The Doc-QA REFERENCE GRAPH, exercised end-to-end with NO provider keys: the agent
 * runs on a deterministic mock gateway. It proves the complete input → output RAG
 * pipeline (clean → split → retrieve → rerank → prompt → agent → fieldExtractor →
 * answerBuilder) RUNS to completion and POPULATES the single output channel `answer`
 * with a CLEAN human-readable answer (no raw AgentResult JSON dump).
 *
 * Provider keys are cleared so the run is reproducible on whichever engine the harness
 * selected (Rust when the native addon is present, else the TS fallback).
 */

const QUESTION = "How does Ailu resume a run after a crash or an approval?";
const DOCUMENTS =
  "<p>Ailu is a stateful, resumable agent graph runtime.</p> It checkpoints after " +
  "every node. Human gates suspend the run cleanly for approval.";

describe("@ailu-ai/graph-sdk — Doc-QA reference graph (offline, deterministic)", () => {
  const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"] as const;
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    for (const key of PROVIDER_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(() => {
    for (const key of PROVIDER_KEYS) {
      if (saved[key] === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = saved[key];
      }
    }
  });

  it("runs the full pipeline to completion and populates the `answer` output channel", async () => {
    const app = buildDocQaReference();
    const runId = "run_doc_qa_reference_test" as RunId;

    const out = await app.run({ question: QUESTION, documents: DOCUMENTS }, { runId });

    // The single input set drove the run to completion.
    expect(out.status).toBe("completed");
    // The single OUTPUT channel is populated and non-empty.
    expect(typeof out.channels.answer).toBe("string");
    expect((out.channels.answer as string).trim().length).toBeGreaterThan(0);
    // The pipeline grounded the answer: it carries the numbered citation block.
    expect(out.channels.answer as string).toContain("Sources:");
    // The answer is CLEAN human-readable text, NOT a raw AgentResult JSON dump.
    const answerText = out.channels.answer as string;
    expect(answerText).not.toContain('{"reasoning"');
    expect(answerText).not.toContain('"requiresHumanReview"');
    expect(answerText).not.toContain('"approvalRequests"');
    expect(answerText.startsWith("{")).toBe(false);
    // The agent's reasoning trace was reduced to just its final-answer text on `finalAnswer`.
    expect(typeof out.channels.finalAnswer).toBe("string");
    expect((out.channels.finalAnswer as string).trim().length).toBeGreaterThan(0);
    expect(out.channels.finalAnswer as string).not.toContain("final:");
    // The clean answer text leads the assembled output (before the Sources block).
    expect(answerText.split("\n\nSources:")[0]).toBe((out.channels.finalAnswer as string));
    // Upstream stages ran: ingestion-prep cleaned + chunked, retrieval ranked the corpus.
    expect(typeof out.channels.cleaned).toBe("string");
    expect((out.channels.cleaned as string).includes("<p>")).toBe(false); // HTML stripped
    expect(Array.isArray(out.channels.chunks)).toBe(true);
    expect(Array.isArray(out.channels.ranked)).toBe(true);
    expect((out.channels.ranked as unknown[]).length).toBeGreaterThan(0);
  });

  it("exposes the ordered pipeline with the catalog carrier on every node", () => {
    const definition = docQaReferenceDefinition();
    const nodeIds = definition.nodes.map((node) => String(node.id));
    expect(nodeIds).toEqual([
      "clean",
      "split",
      "retrieve",
      "rerank",
      "prompt",
      "answer",
      "extract",
      "assemble"
    ]);
    expect(String(definition.entryNodeId)).toBe("clean");

    // Every component node carries `node.metadata.component = { kind, params }`.
    const byId = (id: string) => definition.nodes.find((node) => String(node.id) === id);
    expect(readComponentCarrier(byId("clean")?.metadata)?.kind).toBe("textCleaner");
    expect(readComponentCarrier(byId("split")?.metadata)?.kind).toBe("documentSplitter");
    expect(readComponentCarrier(byId("retrieve")?.metadata)?.kind).toBe("retriever");
    expect(readComponentCarrier(byId("rerank")?.metadata)?.kind).toBe("reranker");
    expect(readComponentCarrier(byId("prompt")?.metadata)?.kind).toBe("promptBuilder");
    expect(readComponentCarrier(byId("extract")?.metadata)?.kind).toBe("fieldExtractor");
    expect(readComponentCarrier(byId("assemble")?.metadata)?.kind).toBe("answerBuilder");
    // The agent node carries `node.metadata.agent` (balanced tier, ragResult channel).
    const agent = readAgentCarrier(byId("answer")?.metadata);
    expect(agent?.tier).toBe("balanced");
    expect(agent?.outputChannel).toBe("ragResult");

    // The definition is recognised as a catalog graph (the API run path key).
    expect(isCatalogGraph(definition)).toBe(true);
  });

  // The carrier-only definition runs on the Rust engine through the catalog run path —
  // the exact seam the control plane uses. Skipped when the native addon is absent.
  (rustEngineAvailable() ? it : it.skip)(
    "runs the carrier-only definition on the Rust engine via runCatalogGraph",
    async () => {
      const definition = docQaReferenceDefinition();
      const outcome = await runCatalogGraph(definition, {
        runId: "run_doc_qa_reference_catalog" as RunId,
        initialData: { question: QUESTION, documents: DOCUMENTS }
      });
      expect(outcome.status).toBe("completed");
      expect(outcome.usedRustEngine).toBe(true);
      const answer = outcome.state.channels.answer;
      expect(typeof answer).toBe("string");
      expect((answer as string).trim().length).toBeGreaterThan(0);
    }
  );
});
