/**
 * Reference pipeline — Doc-QA (retrieval-augmented question answering), end to end.
 *
 * A COMPLETE input → output pipeline composed entirely from the catalog and run on the
 * Rust engine:
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
 * ── OFFLINE vs LIVE ───────────────────────────────────────────────────────────
 *   - AILU_LLM_MOCK=1, no key → the answerer runs on the engine's deterministic offline
 *                               mock, so the whole pipeline is reproducible with no network.
 *   - MISTRAL_API_KEY         → the balanced tier resolves to a concrete Mistral model and
 *                               the answerer makes a real (short) call.
 *
 * Self-verifying: every claim below is checked, and the first failed check throws, so this
 * example doubles as an end-to-end smoke test. Checks are structural (status, channels
 * written, citations present), never the model's wording.
 *
 * Run it:
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:docqa
 */

import {
  buildDocQaReference,
  docQaReferenceDefinition,
  isCatalogGraph,
  runCatalogGraph,
  type RunId
} from "@ailu-ai/graph-sdk";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

const liveKey = process.env.MISTRAL_API_KEY !== undefined && process.env.MISTRAL_API_KEY.length > 0;

const QUESTION = "How does Ailu resume a run after a crash or an approval?";
const DOCUMENTS =
  "<p>Ailu is a stateful, resumable agent graph runtime.</p> It checkpoints after " +
  "every node completion. Human gates suspend the run cleanly for approval.";

console.log(`\nDoc-QA reference pipeline (${liveKey ? "live Mistral" : "offline mock"})\n`);
console.log(`Question: ${QUESTION}\n`);

// ── Run 1: as a CompiledGraph (the runnable SDK object) ──────────────────────
const app = buildDocQaReference();

const out = await app.run({ question: QUESTION, documents: DOCUMENTS }, { runId: "doc-qa-example" as RunId });
const answer = String(out.channels.answer ?? "");

check(out.status === "completed", "the pipeline ran to completion");
check(answer.trim().length > 0, "the `answer` output channel is a non-empty string");
check(answer.includes("Sources:"), "the answer is grounded with a citations block");
check(
  !answer.includes('{"reasoning"') && !answer.trimStart().startsWith("{"),
  "the answer is CLEAN text, not a raw AgentResult JSON dump"
);
check(!String(out.channels.cleaned).includes("<p>"), "the documents were HTML-cleaned");
check(Array.isArray(out.channels.chunks), "the documents were split into chunks");
check((out.channels.ranked as unknown[]).length > 0, "the retriever + reranker ranked the corpus");

console.log(`\n  Answer:\n${answer.split("\n").map((line) => `    ${line}`).join("\n")}\n`);

// ── Run 2: as a carrier-only GraphDefinition through the catalog run path ─────
// This is the exact seam the control plane uses: a plain GraphDefinition whose nodes
// carry node.metadata.component / node.metadata.agent, executed on the Rust engine.
const definition = docQaReferenceDefinition();
check(isCatalogGraph(definition), "the definition is a catalog graph (carrier present on its nodes)");

console.log("Catalog run path (runCatalogGraph on the Rust engine):");
const outcome = await runCatalogGraph(definition, {
  runId: "doc-qa-example-catalog" as RunId,
  initialData: { question: QUESTION, documents: DOCUMENTS }
});
check(outcome.status === "completed", "the carrier-only definition ran to completion");
check(
  typeof outcome.state.channels.answer === "string" && outcome.state.channels.answer.trim().length > 0,
  "the catalog run populated the `answer` output channel"
);

console.log("\nAll checks passed — the Doc-QA reference pipeline runs end to end.");
