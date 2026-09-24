/**
 * Reference pipeline — Doc-QA (retrieval-augmented question answering), end to end.
 *
 * A COMPLETE input → output pipeline composed entirely from the catalog and run on the
 * engine (Rust when the `@ailu-ai/napi` addon is present, else the TS fallback):
 *
 *   INPUT { question, documents }
 *     → clean    (textCleaner)       normalise the raw documents text
 *     → split    (documentSplitter)  chunk it into passages
 *     → retrieve (retriever)         deterministic mock-embedding top-k over the corpus
 *     → rerank   (reranker)          reorder the hits against the question
 *     → prompt   (promptBuilder)     build a grounded prompt from context + question
 *     → answer   (AGENT, balanced)   a grounded RAG answerer writes its AgentResult
 *     → extract  (fieldExtractor)    reduce AgentResult.reasoning to the final answer text
 *     → assemble (answerBuilder)     answer text + numbered citations → OUTPUT { answer }
 *   OUTPUT { answer }
 *
 * Single input set, single output channel.
 *
 * ── OFFLINE vs LIVE (no key required to run) ──────────────────────────────────
 *   - no key            → the answerer runs on a deterministic MOCK gateway, so the
 *                         whole pipeline is reproducible with no network.
 *   - MISTRAL_API_KEY   → on the Rust engine the balanced tier resolves to a concrete
 *                         Mistral model and the answerer makes a real (short) call.
 *
 * Offline and self-verifying: every claim below is asserted — the process exits 1 on
 * the first failed assertion, so this example doubles as an end-to-end smoke test.
 *
 * Run it:
 *   pnpm --filter @ailu-ai/graph-sdk example:docqa
 *   pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/doc-qa-reference.ts
 */

import {
  buildDocQaReference,
  docQaReferenceDefinition,
  isCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type RunId
} from "@ailu-ai/graph-sdk";

// ── Self-verification helpers ────────────────────────────────────────────────
const assert = (condition: boolean, label: string): void => {
  if (!condition) {
    console.error(`✗ ASSERTION FAILED: ${label}`);
    process.exit(1);
  }
  console.log(`  ✓ ${label}`);
};

const liveKey = process.env.MISTRAL_API_KEY !== undefined && process.env.MISTRAL_API_KEY.length > 0;

const QUESTION = "How does Ailu resume a run after a crash or an approval?";
const DOCUMENTS =
  "<p>Ailu is a stateful, resumable agent graph runtime.</p> It checkpoints after " +
  "every node completion. Human gates suspend the run cleanly for approval.";

console.log(`\nDoc-QA reference pipeline (${liveKey ? "LIVE Mistral" : "offline mock"})\n`);
console.log(`Question: ${QUESTION}\n`);

// ── Run 1: as a CompiledGraph (the runnable SDK object) ──────────────────────
const app = buildDocQaReference();
console.log(`Engine: ${app.usesRustEngine ? "Rust (@ailu-ai/napi)" : "TypeScript fallback"}\n`);

const out = await app.run({ question: QUESTION, documents: DOCUMENTS }, { runId: "doc-qa-example" as RunId });

assert(out.status === "completed", "the pipeline ran to completion");
assert(typeof out.channels.answer === "string", "the `answer` output channel is a string");
assert((out.channels.answer as string).trim().length > 0, "the `answer` output channel is non-empty");
assert((out.channels.answer as string).includes("Sources:"), "the answer is grounded with a citations block");
assert(
  !(out.channels.answer as string).includes('{"reasoning"') &&
    !(out.channels.answer as string).trimStart().startsWith("{"),
  "the answer is CLEAN text, not a raw AgentResult JSON dump"
);
assert(!(out.channels.cleaned as string).includes("<p>"), "the documents were HTML-cleaned");
assert(Array.isArray(out.channels.chunks), "the documents were split into chunks");
assert((out.channels.ranked as unknown[]).length > 0, "the retriever + reranker ranked the corpus");

console.log(`\n  Answer:\n${(out.channels.answer as string).split("\n").map((l) => `    ${l}`).join("\n")}\n`);

// ── Run 2: as a carrier-only GraphDefinition through the catalog run path ─────
// This is the exact seam the control plane uses: a plain GraphDefinition whose nodes
// carry node.metadata.component / node.metadata.agent, executed on the Rust engine.
const definition = docQaReferenceDefinition();
assert(isCatalogGraph(definition), "the definition is a catalog graph (carrier present on its nodes)");

if (rustEngineAvailable()) {
  console.log("Catalog run path (runCatalogGraph on the Rust engine):");
  const outcome = await runCatalogGraph(definition, {
    runId: "doc-qa-example-catalog" as RunId,
    initialData: { question: QUESTION, documents: DOCUMENTS }
  });
  assert(outcome.status === "completed", "the carrier-only definition ran to completion on Rust");
  assert(outcome.usedRustEngine, "it executed on the Rust engine");
  assert(
    typeof outcome.state.channels.answer === "string" &&
      (outcome.state.channels.answer as string).trim().length > 0,
    "the catalog run populated the `answer` output channel"
  );
} else {
  console.log("  (native addon absent — skipping the Rust catalog run path; build with scripts/build-napi.sh)");
}

console.log("\nAll assertions passed — the Doc-QA reference pipeline runs end-to-end.");
