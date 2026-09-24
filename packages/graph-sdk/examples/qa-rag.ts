/**
 * Tutorial — Question answering over your documents (governed QA).
 *
 * What you'll learn:
 *   - retrieval QA as a graph: retrieve → answer (an agent node) → cite
 *   - the governance twist classic RAG stacks lack: a conditional edge sends any answer that
 *     is NOT backed by a retrieved source into a human gate ("low-confidence-review") instead
 *     of publishing it
 *   - both paths: a question the corpus covers publishes straight through; a question it does
 *     not cover suspends the run until a human reviews the answer and resumes it
 *
 * The routing decision is made by plain code (which documents matched the question), never by
 * the model's wording, so the tutorial behaves the same offline and with a real key.
 *
 * Self-verifying: every claim is checked and the first failed check throws.
 *
 * Run it offline (the engine's deterministic mock answers "done"):
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:qa
 * With ANTHROPIC_API_KEY set, the agent writes a real answer from the retrieved passages.
 */
import { createGraph, finalAnswer, model } from "@ailu-ai/graph-sdk";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

// ── The corpus: short documents about the Ailu engine itself ─────────────────
type Doc = { id: string; title: string; content: string };

const CORPUS: Doc[] = [
  {
    id: "checkpointing",
    title: "Checkpoints & resumability",
    content:
      "Ailu checkpoints a run after every node completion and state mutation. When a " +
      "process crashes or a run suspends for approval, you resume from the latest checkpoint " +
      "and the run continues exactly where it stopped."
  },
  {
    id: "human-gates",
    title: "Human-approval gates",
    content:
      "A human-gate node suspends the run cleanly (run_suspended) until a person approves. " +
      "Agents never approve their own outputs — approval is always a different principal."
  },
  {
    id: "channels",
    title: "Typed channels",
    content:
      "State flows through declared channels with reducers (replace or append). Channel value " +
      "types flow through the builder into the results of run and resume."
  },
  {
    id: "attestation",
    title: "Attestation & audit",
    content:
      "Every approval decision is recorded with who approved, when, and which subject — an " +
      "attestation trail auditors can replay."
  },
  {
    id: "determinism",
    title: "Deterministic execution",
    content:
      "Graphs execute deterministically by default: same definition, same inputs, same path. " +
      "Conditions are named predicates, never eval'd code."
  },
  {
    id: "streaming",
    title: "Streaming",
    content:
      "Observe values, updates, messages or debug events while a graph executes, and stream " +
      "agent tokens for live chat UIs."
  },
  {
    id: "time-travel",
    title: "Time travel",
    content:
      "Rewind a run to any past checkpoint and branch from there — useful to replay a decision " +
      "with a different approval outcome."
  }
];

// ── Tiny keyword retrieval (whole-word matches — no embeddings needed) ───────
type Source = Doc & { score: number };

const STOP_WORDS = new Set(["the", "and", "for", "how", "does", "what", "who", "why", "when", "after"]);
const MIN_SCORE = 2; // a document must match at least two question terms to count as a source

const terms = (text: string): string[] =>
  text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((term) => term.length > 2 && !STOP_WORDS.has(term));

const retrieve = (question: string): Source[] => {
  const wanted = new Set(terms(question));
  return CORPUS.map((doc) => ({
    ...doc,
    score: terms(`${doc.title} ${doc.content}`).filter((term) => wanted.has(term)).length
  }))
    .filter((hit) => hit.score >= MIN_SCORE)
    .sort((a, b) => b.score - a.score)
    .slice(0, 2);
};

// A published answer must carry at least one citation marker like [doc:checkpointing].
const CITATION = /\[doc:[a-z0-9-]+\]/;

// ── The graph ────────────────────────────────────────────────────────────────
const app = createGraph({ name: "qa-over-docs" })
  .channel("question", { type: "string", default: "" })
  .channel("sources", { type: "json", default: [] as Source[] })
  .channel("answer", { type: "string", default: "" })
  .channel("published", { type: "boolean", default: false })
  // 1. Retrieve: keep only the documents that match the question.
  .node("retrieve", async (_input, state) => ({ sources: retrieve(state.channels.question) }))
  // 2. Answer: the agent sees only the question and the retrieved sources.
  .agentNode("qa-agent", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: {
      system:
        "Answer the question using only the sources in state. " +
        "If the sources do not answer it, say that you cannot answer."
    },
    visibleChannels: ["question", "sources"],
    maxIterations: 3,
    outputChannel: "qaResult"
  })
  // 3. Cite: attach a citation for every source the answer was grounded on.
  .node("cite", async (_input, state) => {
    const citations = state.channels.sources.map((source) => `[doc:${source.id}]`).join(" ");
    return { answer: `${finalAnswer(state.channels.qaResult)} ${citations}`.trim() };
  })
  .humanGate("low-confidence-review")
  .node("publish-answer", async () => ({ published: true }))
  .edge("retrieve", "qa-agent")
  .edge("qa-agent", "cite")
  // The governance twist: an answer without a citation never publishes itself.
  .conditionalEdge("cite", "publish-answer", "hasCitation", (s) => CITATION.test(s.channels.answer))
  .conditionalEdge("cite", "low-confidence-review", "lacksCitation", (s) => !CITATION.test(s.channels.answer))
  .edge("low-confidence-review", "publish-answer")
  .compile();

// ── Run 1: the corpus covers the question → cited answer, published directly ──
const COVERED = "How does Ailu resume a run after a crash or an approval?";
console.log(`\nRun 1 — ${COVERED}`);

const cited = await app.run({ question: COVERED });

check(cited.status === "completed", "the run completed without a human in the loop");
check(cited.channels.sources[0]?.id === "checkpointing", "retrieval ranked the checkpointing doc first");
check(finalAnswer(cited.channels.qaResult).length > 0, "the agent wrote an answer");
check(cited.channels.answer.includes("[doc:checkpointing]"), "the answer cites [doc:checkpointing]");
check(cited.channels.published, "the cited answer was published");
console.log(`\n  Answer: ${cited.channels.answer}\n`);

// ── Run 2: the corpus does not cover the question → human review first ───────
const UNCOVERED = "What is the boiling point of water on Mars?";
console.log(`Run 2 — ${UNCOVERED}`);

const suspended = await app.run({ question: UNCOVERED });

check(suspended.channels.sources.length === 0, "no document matched the question");
check(suspended.status === "suspended", "the uncited answer did NOT publish itself");
check(
  String(suspended.currentNodeId) === "low-confidence-review",
  "the run is paused at the low-confidence-review human gate"
);
check(!suspended.channels.published, "nothing was published before the review");

// A human reviews the answer out of band, then resumes the run.
const reviewed = await app.resume(suspended.runId);
check(reviewed.status === "completed", "the run completed after human review + resume");
check(reviewed.channels.published, "the reviewed answer was published");
console.log(`\n  Answer (human-reviewed): ${reviewed.channels.answer}\n`);

console.log("All checks passed — governed QA behaves as documented.");
