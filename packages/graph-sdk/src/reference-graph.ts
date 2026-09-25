/**
 * The Doc-QA REFERENCE GRAPH — a complete input → output retrieval-augmented
 * question-answering pipeline, composed entirely from the catalog (pure components +
 * one prebuilt-style agent), authored once and runnable two ways:
 *
 *   1. as a {@link CompiledGraph} via {@link buildDocQaReference} — runs on the Rust engine;
 *   2. as a plain {@link GraphDefinition} via {@link docQaReferenceDefinition} — every
 *      node carries the shared `node.metadata.component` / `node.metadata.agent`
 *      carrier, so the control plane can persist it, the Studio can render it, and the
 *      catalog run path (`runCatalogGraph`) can execute it on the Rust engine.
 *
 * ── THE PIPELINE ──────────────────────────────────────────────────────────────
 *   INPUT { question, documents }
 *     → clean      (textCleaner)        normalise the raw documents text
 *     → split      (documentSplitter)   chunk it into passages
 *     → retrieve   (retriever)          deterministic mock-embedding top-k over the corpus
 *     → rerank     (reranker)           reorder the hits against the question
 *     → prompt     (promptBuilder)      build a grounded prompt from context + question
 *     → answer     (AGENT, balanced)    a grounded RAG answerer writes its AgentResult
 *     → extract    (fieldExtractor)     reduce AgentResult.reasoning to the final answer text
 *     → assemble   (answerBuilder)      answer text + numbered citations → OUTPUT { answer }
 *   OUTPUT { answer }
 *
 * Single input set, single output channel. Runs on Mistral's balanced-tier model
 * (`MISTRAL_API_KEY`), or offline and deterministic with `AILU_LLM_MOCK=1`.
 *
 * The retriever scores against a fixed corpus baked into its params (the Rust
 * `retriever` component's `docs` are configuration, not a channel). The `documents`
 * INPUT channel feeds the clean → split ingestion-prep stages so the graph exercises a
 * real document-preparation front-end; the corpus the retriever ranks is the same
 * knowledge the documents describe, kept in params so the run is fully reproducible.
 */

import type { GraphDefinition } from "@ailu-ai/graph-core";
import type { LLMGateway, ModelTier } from "@ailu-ai/llm-gateway";

import { createGraph } from "./builder.js";
import type { CompiledGraph } from "./compiled-graph.js";
import { components, type RetrieverDoc } from "./components.js";

/** A grounded prompt template: the retrieved context, then the question. */
const PROMPT_TEMPLATE =
  "Answer the question using ONLY the context below. Cite the passage ids you rely " +
  "on. If the context does not contain the answer, say you do not know.\n\n" +
  "Context:\n{{ranked}}\n\nQuestion: {{question}}\n\nAnswer:";

/** The grounded RAG answerer's system prompt (mirrors the prebuilt `ragAnswerer`). */
const RAG_SYSTEM_PROMPT =
  "You are a retrieval-augmented answerer. You are given a question and a set of " +
  "retrieved context passages. Answer the question using only the provided context. " +
  "If the context does not contain the answer, say you do not know. Cite the passage " +
  "ids you relied on. Respond with the answer only.";

/** The default knowledge corpus the retriever ranks against. */
export const DEFAULT_REFERENCE_CORPUS: RetrieverDoc[] = [
  {
    id: "checkpointing",
    content:
      "Ailu checkpoints a run after every node completion and state mutation, so a " +
      "crashed or suspended run resumes from the latest checkpoint and continues exactly " +
      "where it stopped."
  },
  {
    id: "human-gates",
    content:
      "A human-gate node suspends the run cleanly until a person approves. Agents never " +
      "approve their own outputs; approval is always a different principal."
  },
  {
    id: "determinism",
    content:
      "Graphs execute deterministically by default: same definition, same inputs, same " +
      "path. Conditions are named predicates, never eval'd code."
  },
  {
    id: "channels",
    content:
      "State flows through declared channels with reducers, replace or append. Channel " +
      "value types flow through the builder into the results of run and resume."
  },
  {
    id: "events",
    content:
      "The runtime emits a lifecycle event for every node transition, so every run is " +
      "fully observable and auditable."
  }
];

/** Options for {@link buildDocQaReference} / {@link docQaReferenceDefinition}. */
export type DocQaReferenceOptions = {
  /** @deprecated Ignored: the engine calls the model itself. Use `AILU_LLM_MOCK=1` to run offline. */
  llm?: LLMGateway;
  /** The corpus the retriever ranks against. Defaults to {@link DEFAULT_REFERENCE_CORPUS}. */
  corpus?: RetrieverDoc[];
  /** How many documents the retriever keeps before reranking. Defaults to 3. */
  k?: number;
  /** The answerer's capability tier. Defaults to `"balanced"`. */
  tier?: ModelTier;
  /** The answerer's provider. Defaults to `"mistral"`. */
  provider?: "openai" | "anthropic" | "mistral";
};

/**
 * Build the Doc-QA reference graph as a runnable {@link CompiledGraph}. Drive it with
 * a single input set and read the single output channel:
 *
 * ```ts
 * const app = buildDocQaReference();
 * const out = await app.run({ question: "How does Ailu resume after a crash?", documents: "…" });
 * console.log(out.channels.answer);
 * ```
 */
export const buildDocQaReference = (options: DocQaReferenceOptions = {}): CompiledGraph => {
  const corpus = options.corpus ?? DEFAULT_REFERENCE_CORPUS;
  const k = options.k ?? 3;
  const tier = options.tier ?? "balanced";
  const provider = options.provider ?? "mistral";

  return createGraph({ name: "doc-qa-reference", id: "doc-qa-reference" })
    // INPUT channels.
    .channel("question", { type: "string", default: "" })
    .channel("documents", { type: "string", default: "" })
    // Intermediate channels (the pipeline's working state).
    .channel("cleaned", { type: "string", default: "" })
    .channel("chunks", { type: "json", default: [] })
    .channel("retrieved", { type: "json", default: [] })
    .channel("ranked", { type: "json", default: [] })
    .channel("prompt", { type: "string", default: "" })
    .channel("ragResult", { type: "agentResult", reducer: "replace" })
    .channel("finalAnswer", { type: "string", default: "" })
    // OUTPUT channel.
    .channel("answer", { type: "string", default: "" })
    // 1. Normalise the raw documents text.
    .component(
      "clean",
      components.textCleaner({
        from: "documents",
        into: "cleaned",
        stripHtml: true,
        collapseWhitespace: true,
        trim: true
      })
    )
    // 2. Chunk the cleaned text into passages.
    .component(
      "split",
      components.documentSplitter({ from: "cleaned", into: "chunks", by: "sentences", size: 2 })
    )
    // 3. Deterministic mock-embedding retrieval over the corpus, ranked by the question.
    .component(
      "retrieve",
      components.retriever({ query: "question", into: "retrieved", k, docs: corpus })
    )
    // 4. Rerank the hits against the question.
    .component(
      "rerank",
      components.reranker({ from: "retrieved", into: "ranked", query: "question" })
    )
    // 5. Build a grounded prompt: the reranked context block + the question.
    .component(
      "prompt",
      components.promptBuilder({ template: PROMPT_TEMPLATE, into: "prompt" })
    )
    // 6. The grounded RAG answerer (balanced tier) writes its AgentResult to `ragResult`.
    .agentNode("answer", {
      model: { provider, tier },
      prompt: { system: RAG_SYSTEM_PROMPT },
      name: "ragAnswerer",
      description: "Answers a question grounded in the retrieved, reranked context.",
      outputChannel: "ragResult"
    })
    // 7. Reduce the agent's AgentResult to a CLEAN final-answer string. The agent
    //    wrote a full AgentResult object to `ragResult`; its `reasoning` field is a
    //    trace whose final line is `final:<answer>`. The pure `fieldExtractor` follows
    //    the `reasoning` path and, with `finalOnly`, keeps only the text after the last
    //    `final:` marker — turning the object into the human-readable answer text. Pure
    //    and deterministic.
    .component(
      "extract",
      components.fieldExtractor({
        from: "ragResult",
        into: "finalAnswer",
        path: "reasoning",
        finalOnly: true
      })
    )
    // 8. Assemble the clean answer text + numbered citations into the OUTPUT channel.
    //    answerBuilder renders `finalAnswer` as `{{answer}}` and the reranked passages
    //    as a numbered `{{citations}}` block — a grounded, cited, human-readable answer
    //    in the single `answer` output channel (no raw AgentResult JSON dump).
    .component(
      "assemble",
      components.answerBuilder({
        from: "finalAnswer",
        into: "answer",
        contextFrom: "ranked",
        template: "{{answer}}\n\nSources:\n{{citations}}"
      })
    )
    .edge("clean", "split")
    .edge("split", "retrieve")
    .edge("retrieve", "rerank")
    .edge("rerank", "prompt")
    .edge("prompt", "answer")
    .edge("answer", "extract")
    .edge("extract", "assemble")
    .entry("clean")
    .compile();
};

/**
 * The Doc-QA reference graph as a plain {@link GraphDefinition} carrying the shared
 * `node.metadata.component` / `node.metadata.agent` carrier on every node. This is the
 * form the control plane persists, the Studio renders, and `runCatalogGraph` executes
 * on the Rust engine. Pure data — no handler closures, no LLM gateway.
 */
export const docQaReferenceDefinition = (options: DocQaReferenceOptions = {}): GraphDefinition =>
  buildDocQaReference(options).definition;
