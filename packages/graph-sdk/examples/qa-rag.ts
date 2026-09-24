/**
 * Tutorial — Question answering over your documents (governed QA).
 *
 * What you'll learn:
 *   - retrieval QA with an agent node: search → fetch → answer with a citation
 *   - the governance twist classic RAG stacks lack: a conditional edge routes any
 *     answer WITHOUT a citation into a human gate ("low-confidence-review") instead
 *     of publishing it blindly
 *   - both paths are demonstrated: a cited answer publishes straight through; an
 *     uncited answer suspends the run until a human resumes it
 *
 * Offline and self-verifying: the LLM is a scripted mock (no API key), and every
 * claim below is asserted — the process exits 1 on the first failed assertion, so
 * this tutorial doubles as an end-to-end test.
 *
 * Run it:
 *   pnpm --filter @ailu/graph-sdk example:qa
 */
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  type LLMGateway,
  type LLMResponse,
  type ToolId
} from "@ailu/graph-sdk";

// ── Self-verification helpers ────────────────────────────────────────────────
const assert = (condition: boolean, label: string): void => {
  if (!condition) {
    console.error(`✗ ASSERTION FAILED: ${label}`);
    process.exit(1);
  }
  console.log(`  ✓ ${label}`);
};

// ── The corpus: short documents about the Ailu engine itself ─────────────
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

// ── Tiny keyword retrieval (term frequency — no embeddings needed offline) ──
const tokenize = (text: string): string[] =>
  text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((term) => term.length > 2);

const scoreDoc = (doc: Doc, terms: string[]): number => {
  const haystack = `${doc.title} ${doc.content}`.toLowerCase();
  return terms.reduce((score, term) => score + (haystack.split(term).length - 1), 0);
};

// ── Scripted mock LLM turns ──────────────────────────────────────────────────
let toolUseSeq = 0;
const toolTurn = (name: string, input: Record<string, unknown>): LLMResponse => ({
  content: "",
  toolCalls: [{ id: `tu_${(toolUseSeq += 1)}`, name, input }],
  stopReason: "tool_use",
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});

const finalTurn = (content: string): LLMResponse => ({
  content,
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});

const scripted = (responses: LLMResponse[]): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(new MockLLMProviderAdapter({ provider: "anthropic", responses }));
  return gateway;
};

// An answer is "confident" when it carries a citation marker like [doc:checkpointing].
const CITATION = /\[doc:[a-z0-9-]+\]/;

// ── Graph factory: same graph, swappable script, fresh tool counters ─────────
const buildQaGraph = (script: LLMResponse[]) => {
  const counters = { search: 0, fetch: 0 };
  const captured = { topHitId: "" };
  const passthrough = { parse: (value: unknown) => value };

  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "search_documents" as ToolId,
      name: "search_documents",
      description: "Keyword search over the corpus. Returns the top-3 documents with scores.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: {
        type: "object",
        properties: { query: { type: "string" } },
        required: ["query"]
      }
    },
    async (input: unknown) => {
      counters.search += 1;
      const { query } = input as { query: string };
      const terms = tokenize(query);
      const hits = CORPUS.map((doc) => ({ id: doc.id, title: doc.title, score: scoreDoc(doc, terms) }))
        .sort((a, b) => b.score - a.score)
        .slice(0, 3);
      captured.topHitId = hits[0]?.id ?? "";
      return { hits };
    }
  );
  tools.register(
    {
      id: "fetch_document" as ToolId,
      name: "fetch_document",
      description: "Returns the full content of a document by id.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: {
        type: "object",
        properties: { id: { type: "string" } },
        required: ["id"]
      }
    },
    async (input: unknown) => {
      counters.fetch += 1;
      const { id } = input as { id: string };
      return CORPUS.find((doc) => doc.id === id) ?? { error: `document_not_found:${id}` };
    }
  );

  const app = createGraph({ name: "qa-over-docs" })
    .channel("question", { type: "string", default: "" })
    .channel("answer", { type: "string", default: "" })
    .channel("published", { type: "boolean", default: false })
    .agentNode("qa-agent", {
      llm: scripted(script),
      prompt: { system: "Answer using the document tools. Cite your source as [doc:<id>]." },
      tools,
      maxIterations: 5,
      outputChannel: "qaResult"
    })
    // Pull the agent's FINAL line out of its trace into the typed `answer` channel.
    .node("extract-answer", async (_input, state) => {
      const final = /^final:(.*)$/m.exec(state.channels.qaResult.reasoning);
      return { answer: (final?.[1] ?? "").trim() };
    })
    .humanGate("low-confidence-review")
    .node("publish-answer", async () => ({ published: true }))
    .edge("qa-agent", "extract-answer")
    // The governance twist: an uncited answer never publishes itself.
    .conditionalEdge("extract-answer", "publish-answer", "hasCitation", (s) =>
      CITATION.test(s.channels.answer)
    )
    .conditionalEdge("extract-answer", "low-confidence-review", "lacksCitation", (s) =>
      !CITATION.test(s.channels.answer)
    )
    .edge("low-confidence-review", "publish-answer")
    .compile();

  return { app, counters, captured };
};

// ── The question and the two scripts (the agent behaves; then it hallucinates) ─
const QUESTION = "How does Ailu resume a run after a crash or an approval?";

const retrievalTurns: LLMResponse[] = [
  toolTurn("search_documents", { query: "resume run crash checkpoint approval" }),
  toolTurn("fetch_document", { id: "checkpointing" })
];

const citedScript = [
  ...retrievalTurns,
  finalTurn(
    "FINAL: Ailu checkpoints after every node completion and state mutation, so a crashed " +
      "or suspended run resumes from the latest checkpoint [doc:checkpointing]."
  )
];

const uncitedScript = [
  ...retrievalTurns,
  finalTurn("FINAL: It probably resumes from some saved state, but I could not ground this.")
];

// ── Run 1: a grounded, cited answer publishes without human intervention ─────
console.log(`\nQuestion: ${QUESTION}\n`);
console.log("Run 1 — the agent answers WITH a citation:");

const cited = buildQaGraph(citedScript);
const citedRun = await cited.app.run({ question: QUESTION });

assert(citedRun.status === "completed", "cited run completed without suspension");
assert(cited.counters.search === 1, "search_documents was called exactly once");
assert(cited.counters.fetch === 1, "fetch_document was called exactly once");
assert(cited.captured.topHitId === "checkpointing", "retrieval ranked the right document first");
assert(CITATION.test(citedRun.channels.answer), "the answer carries a citation marker");
assert(citedRun.channels.answer.includes("[doc:checkpointing]"), "it cites [doc:checkpointing]");
assert(citedRun.channels.published, "the cited answer was auto-published");
console.log(`\n  Answer: ${citedRun.channels.answer}\n`);

// ── Run 2: an uncited answer suspends at the low-confidence human gate ───────
console.log("Run 2 — the agent answers WITHOUT a citation:");

const uncited = buildQaGraph(uncitedScript);
const suspended = await uncited.app.run({ question: QUESTION });

assert(suspended.status === "suspended", "uncited run suspended instead of publishing");
assert(
  String(suspended.currentNodeId) === "low-confidence-review",
  "it is paused at the low-confidence-review human gate"
);
assert(uncited.counters.search === 1, "search_documents was called exactly once");
assert(uncited.counters.fetch === 1, "fetch_document was called exactly once");

// A human reviews the uncited answer out-of-band, then resumes the run.
const reviewed = await uncited.app.resume(suspended.runId);
assert(reviewed.status === "completed", "uncited run completed after human review + resume");
assert(reviewed.channels.published, "the reviewed answer was published on resume");
console.log(`\n  Answer (human-reviewed): ${reviewed.channels.answer}\n`);

console.log("All assertions passed — governed QA behaves as documented.");
