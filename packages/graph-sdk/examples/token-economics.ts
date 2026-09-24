/**
 * Token economics — "Claude-Code-style monolithic context" vs "Ailu governed RAG agents".
 *
 * ── WHAT THIS IS (and what it is NOT) ────────────────────────────────────────
 * This harness does NOT instrument the real Claude Code product. It models TWO
 * CONTEXT STRATEGIES for the SAME multi-turn task and counts the *prompt tokens*
 * each strategy would emit, using one documented estimator. We count the exact
 * same content under both strategies, so the headline number — the REDUCTION
 * RATIO — is robust to the choice of tokenizer (any monotonic estimator gives a
 * similar ratio). The absolute token and dollar figures are illustrative.
 *
 * The estimator is the SAME one the Ailu engine uses for its own working-memory
 * accounting: estTokens(s) = max(1, ceil(s.length / 4)). See
 * `crates/agents-core` working-memory (token estimate = max(1, ceil(chars/4));
 * compression keeps floor(len/2) + an LLM summary). Using the engine's own model
 * keeps this analysis consistent with how the runtime actually budgets context.
 *
 * ── THE TWO STRATEGIES ───────────────────────────────────────────────────────
 * Strategy A — "monolithic / Claude-Code-style":
 *   Every turn re-sends (system prompt) + (the ENTIRE corpus, as if every file
 *   were pasted into context) + (the full conversation history so far) +
 *   (the tool schemas). No prompt caching. Tokens are summed across all turns.
 *
 * Strategy B — "Ailu governed":
 *   - Turn 1 pays the system+tools prefix at full price; every later turn counts
 *     that prefix at the CACHE-READ rate (~0.1x), mirroring the gateway's
 *     `cache_control` ephemeral breakpoints on the system block + last tool
 *     (see `crates/llm-gateway/src/anthropic.rs`).
 *   - Instead of the whole corpus, each turn retrieves only the top-k relevant
 *     chunks via the qa-rag-style keyword retriever (see `examples/qa-rag.ts`).
 *   - Conversation history is compressed by the working-memory rule once it
 *     exceeds a budget: keep floor(len/2) of the most recent turns verbatim,
 *     replace the older half with a short summary (observation/text collapse).
 *   - Old ReAct tool observations are masked after N turns (observation masking):
 *     their bulky bodies are dropped, a one-line placeholder remains.
 *
 * Self-verifying: asserts the expected ordering (total_B < total_A) and a sane
 * reduction range, then exits 1 on any violation.
 *
 * Run it:
 *   pnpm --filter @ailu/graph-sdk exec node --import tsx examples/token-economics.ts
 *   pnpm --filter @ailu/graph-sdk example:tokens
 */

// ── Self-verification helpers ────────────────────────────────────────────────
const assert = (condition: boolean, label: string): void => {
  if (!condition) {
    console.error(`✗ ASSERTION FAILED: ${label}`);
    process.exit(1);
  }
};

// ── The pure token estimator (the engine's own chars/4 model) ────────────────
/**
 * estTokens(s) = max(1, ceil(s.length / 4)). This is the exact estimate the
 * Ailu engine's working memory uses to budget context, so the numbers here
 * line up with the runtime's accounting. It is a deliberate approximation — a
 * real tokenizer (e.g. Anthropic's `count_tokens`) would differ in absolute
 * terms, but the REDUCTION RATIO (same content counted under both strategies)
 * is what we report and that is stable across estimators.
 */
const estTokens = (s: string): number => Math.max(1, Math.ceil(s.length / 4));

// ── Representative pricing (ILLUSTRATIVE — verify against current pricing) ────
// Opus-tier input pricing, used only for the illustrative cost line. Cache reads
// bill at ~0.1x base input; we do not charge output here (both strategies emit
// the same completions, so output cancels out of the comparison).
//   verify against current pricing at https://platform.claude.com/docs/en/pricing
const OPUS_INPUT_USD_PER_MTOK = 5.0; // $ per 1,000,000 input tokens
const CACHE_READ_MULTIPLIER = 0.1; // cache-read input ≈ 0.1x base input price
const usd = (tokens: number): number => (tokens / 1_000_000) * OPUS_INPUT_USD_PER_MTOK;

// ── The corpus: short docs about the Ailu engine (extends qa-rag's corpus) ─
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
      "Agents never approve their own outputs — approval is always a different principal, and " +
      "the decision is attested with who, when, and the subject reviewed."
  },
  {
    id: "channels",
    title: "Typed channels",
    content:
      "State flows through declared channels with reducers (replace or append). Channel value " +
      "types flow through the builder into the results of run and resume, so the graph stays " +
      "type-safe end to end."
  },
  {
    id: "attestation",
    title: "Attestation & audit",
    content:
      "Every approval decision is recorded with who approved, when, and which subject — an " +
      "attestation trail auditors can replay long after the run completed."
  },
  {
    id: "determinism",
    title: "Deterministic execution",
    content:
      "Graphs execute deterministically by default: same definition, same inputs, same path. " +
      "Conditions are named predicates, never eval'd code, so a replayed run takes the same branch."
  },
  {
    id: "streaming",
    title: "Streaming",
    content:
      "Observe values, updates, messages or debug events while a graph executes, and stream " +
      "agent tokens for live chat UIs without breaking the checkpoint contract."
  },
  {
    id: "time-travel",
    title: "Time travel",
    content:
      "Rewind a run to any past checkpoint and branch from there — useful to replay a decision " +
      "with a different approval outcome and compare the two trajectories."
  },
  {
    id: "fan-out",
    title: "Fan-out & send",
    content:
      "A node can emit multiple Send commands to fan a task out across many parallel branches; " +
      "each branch checkpoints independently and the results merge back through an append reducer."
  },
  {
    id: "tool-nodes",
    title: "Tool nodes",
    content:
      "Tool nodes execute the tool calls emitted by the last AI message. Tools flagged " +
      "requiresApproval suspend the run via a dynamic interrupt instead of executing immediately."
  },
  {
    id: "react",
    title: "ReAct agents",
    content:
      "The ReAct agent loops reason → act → observe until it answers or hits its step budget. " +
      "Old observations are masked after a few turns so the context window does not balloon."
  },
  {
    id: "working-memory",
    title: "Working memory & compression",
    content:
      "Working memory keeps a short-term buffer with a token budget. When the buffer overflows it " +
      "keeps the most recent half verbatim and replaces the older half with an LLM summary."
  },
  {
    id: "llm-gateway",
    title: "Prompt-cache-aware LLM gateway",
    content:
      "All LLM calls route through the gateway. It marks the system block and the last tool as " +
      "cache_control ephemeral, so the stable prefix is written once and read at ~0.1x on later turns."
  },
  {
    id: "rag-pipeline",
    title: "RAG pipeline",
    content:
      "The retrieval pipeline scores documents against the query and returns only the top-k chunks, " +
      "so the model sees the few relevant passages instead of the entire corpus on every turn."
  },
  {
    id: "supervisor",
    title: "Supervisor pattern",
    content:
      "A supervisor agent routes work to specialist sub-agents and aggregates their results, keeping " +
      "each sub-agent's context scoped to its own slice of the task."
  },
  {
    id: "checkpointer-pg",
    title: "Postgres checkpointer",
    content:
      "The Postgres checkpointer persists every checkpoint durably so a run survives a full process " +
      "restart; the in-memory checkpointer is the fast default for tests and local development."
  },
  {
    id: "events",
    title: "Event vocabulary",
    content:
      "The runtime emits an event for every node lifecycle transition — started, completed, " +
      "suspended, resumed, failed — so an observer can reconstruct exactly what happened and when."
  }
];

// ── The fixed system prompt + a small tool schema (the cacheable prefix) ─────
const SYSTEM_PROMPT =
  "You are the Ailu documentation agent. Answer questions about the Ailu agent-graph " +
  "engine using the retrieval tools. Always cite the document you used as [doc:<id>]. Never " +
  "approve your own outputs; route any sensitive action through a human-approval gate. Be precise " +
  "and concise, and prefer grounded answers over speculation.";

// A small tool schema, serialized the way the gateway would render it on the wire.
const TOOL_SCHEMA = [
  {
    name: "search_documents",
    description: "Keyword search over the Ailu docs corpus. Returns the top matching documents with scores.",
    input_schema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] }
  },
  {
    name: "fetch_document",
    description: "Return the full content of a single document by its id.",
    input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] }
  }
];

const PREFIX_TEXT = SYSTEM_PROMPT + "\n\n" + JSON.stringify(TOOL_SCHEMA);
const PREFIX_TOKENS = estTokens(PREFIX_TEXT);

// ── Tiny keyword retrieval (same shape as examples/qa-rag.ts) ────────────────
const tokenize = (text: string): string[] =>
  text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((term) => term.length > 2);

const scoreDoc = (doc: Doc, terms: string[]): number => {
  const haystack = `${doc.title} ${doc.content}`.toLowerCase();
  return terms.reduce((score, term) => score + (haystack.split(term).length - 1), 0);
};

const TOP_K = 3;
const retrieve = (query: string): Doc[] => {
  const terms = tokenize(query);
  return [...CORPUS]
    .map((doc) => ({ doc, score: scoreDoc(doc, terms) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, TOP_K)
    .map((hit) => hit.doc);
};

// ── The representative 8-turn session ────────────────────────────────────────
const QUESTIONS: string[] = [
  "How does Ailu resume a run after a crash or an approval?",
  "Who is allowed to approve an agent's output, and how is the decision recorded?",
  "How do typed channels and reducers keep the graph type-safe?",
  "What makes graph execution deterministic and replayable?",
  "How does the LLM gateway use prompt caching to cut token cost?",
  "How does the RAG pipeline decide which documents the model sees?",
  "How does working memory compress a long conversation history?",
  "How does the ReAct loop keep its context window from ballooning?"
];

const TURNS = QUESTIONS.length;

// A representative model answer per turn (used only to grow the conversation
// history that BOTH strategies carry; output tokens are excluded from the cost
// comparison since they are identical across strategies).
const answerFor = (q: string, cited: Doc): string =>
  `Q: ${q}\nA: ${cited.title} — ${cited.content} [doc:${cited.id}]`;

// A bulky ReAct tool observation: the full fetched doc body the agent "saw".
const observationFor = (doc: Doc): string =>
  `OBSERVATION fetch_document(${doc.id}) => ${doc.title}: ${doc.content}`;

// ── Working-memory compression (the engine's rule) ───────────────────────────
// Keep floor(len/2) most-recent turns verbatim; replace the older half with a
// short summary line. Budget is expressed in turns of accumulated history.
const HISTORY_BUDGET_TURNS = 4;

type HistoryEntry = { question: string; answer: string };

const compressHistory = (history: HistoryEntry[]): string => {
  if (history.length <= HISTORY_BUDGET_TURNS) {
    return history.map((h) => `${h.question}\n${h.answer}`).join("\n\n");
  }
  const keep = Math.floor(history.length / 2);
  const older = history.slice(0, history.length - keep);
  const recent = history.slice(history.length - keep);
  const summary = `SUMMARY of ${older.length} earlier turns: covered ${older
    .map((h) => h.question.replace(/\?$/, ""))
    .join("; ")}.`;
  return [summary, ...recent.map((h) => `${h.question}\n${h.answer}`)].join("\n\n");
};

// ── Observation masking (ReAct) ──────────────────────────────────────────────
// After OBSERVATION_MASK_AFTER turns, an observation's bulky body is dropped and
// a one-line placeholder is kept instead.
const OBSERVATION_MASK_AFTER = 2;

const maskedObservations = (observations: { turn: number; text: string; docId: string }[], currentTurn: number): string =>
  observations
    .map((o) =>
      currentTurn - o.turn >= OBSERVATION_MASK_AFTER
        ? `OBSERVATION fetch_document(${o.docId}) => [masked: ${o.text.length} chars elided]`
        : o.text
    )
    .join("\n");

// ── The whole corpus serialized once (Strategy A re-sends this every turn) ───
const FULL_CORPUS_TEXT = CORPUS.map((d) => `# ${d.title} [${d.id}]\n${d.content}`).join("\n\n");
const FULL_CORPUS_TOKENS = estTokens(FULL_CORPUS_TEXT);

// ── Simulate both strategies turn by turn ────────────────────────────────────
type TurnRow = { turn: number; tokensA: number; tokensB: number };

const rows: TurnRow[] = [];

// Savings attribution buckets for Strategy B.
let savedByCache = 0; // prefix tokens billed at 0.1x instead of full on turns >= 2
let savedByRetrieval = 0; // full corpus avoided in favor of top-k chunks
let savedByCompression = 0; // history compressed once it exceeds the budget
let savedByMasking = 0; // old observations masked to a one-line placeholder

const historyLog: HistoryEntry[] = [];
const allObservations: { turn: number; text: string; docId: string }[] = [];

for (let turn = 1; turn <= TURNS; turn += 1) {
  const question = QUESTIONS[turn - 1] ?? "";
  const hits = retrieve(question);
  const citedDoc = hits[0] ?? CORPUS[0];
  if (citedDoc === undefined) {
    assert(false, "corpus is non-empty");
    break;
  }

  // Conversation history as it stands BEFORE this turn's answer is appended.
  const rawHistoryText = historyLog.map((h) => `${h.question}\n${h.answer}`).join("\n\n");
  const rawHistoryTokens = historyLog.length === 0 ? 0 : estTokens(rawHistoryText);

  // ── Strategy A: prefix + full corpus + full raw history + tool schema ──────
  // (Prefix already folds in the tool schema; corpus & raw history re-sent in full,
  //  no caching.)
  const tokensA = PREFIX_TOKENS + FULL_CORPUS_TOKENS + rawHistoryTokens;

  // ── Strategy B: cached prefix + top-k chunks + compressed history + masked obs ─
  const prefixBilledTokens = turn === 1 ? PREFIX_TOKENS : Math.ceil(PREFIX_TOKENS * CACHE_READ_MULTIPLIER);

  const retrievedText = hits.map((d) => `# ${d.title} [${d.id}]\n${d.content}`).join("\n\n");
  const retrievedTokens = estTokens(retrievedText);

  const compressedHistoryText = historyLog.length === 0 ? "" : compressHistory(historyLog);
  const compressedHistoryTokens = historyLog.length === 0 ? 0 : estTokens(compressedHistoryText);

  const maskedObsText = allObservations.length === 0 ? "" : maskedObservations(allObservations, turn);
  const rawObsText = allObservations.length === 0 ? "" : allObservations.map((o) => o.text).join("\n");
  const maskedObsTokens = allObservations.length === 0 ? 0 : estTokens(maskedObsText);
  const rawObsTokens = allObservations.length === 0 ? 0 : estTokens(rawObsText);

  const tokensB = prefixBilledTokens + retrievedTokens + compressedHistoryTokens + maskedObsTokens;

  // ── Attribute B's savings vs the monolithic baseline for this turn ─────────
  // Cache: the prefix would have cost full price every turn under A; B pays 0.1x after turn 1.
  savedByCache += turn === 1 ? 0 : PREFIX_TOKENS - prefixBilledTokens;
  // Retrieval: A sends the whole corpus; B sends only the top-k chunks.
  savedByRetrieval += FULL_CORPUS_TOKENS - retrievedTokens;
  // Compression: A carries raw history; B compresses it once it overflows the budget.
  savedByCompression += rawHistoryTokens - compressedHistoryTokens;
  // Masking: A carries every raw observation forever (folded into its raw history /
  // corpus re-send); B masks old ones. We measure the masking delta directly.
  savedByMasking += rawObsTokens - maskedObsTokens;

  rows.push({ turn, tokensA, tokensB });

  // Append this turn's answer to history and record the tool observation.
  historyLog.push({ question, answer: answerFor(question, citedDoc) });
  allObservations.push({ turn, text: observationFor(citedDoc), docId: citedDoc.id });
}

// ── Totals ───────────────────────────────────────────────────────────────────
const totalA = rows.reduce((sum, r) => sum + r.tokensA, 0);
const totalB = rows.reduce((sum, r) => sum + r.tokensB, 0);
const reductionPct = ((totalA - totalB) / totalA) * 100;
const savingsBucketTotal = savedByCache + savedByRetrieval + savedByCompression + savedByMasking;

// ── Output: the table ─────────────────────────────────────────────────────────
const pad = (s: string | number, w: number): string => String(s).padStart(w);

console.log("");
console.log("Token economics: monolithic (A) vs Ailu governed RAG (B)");
console.log(`  estimator: estTokens(s) = max(1, ceil(s.length / 4))   (the engine's chars/4 model)`);
console.log(`  task: ${TURNS}-turn Q&A over a ${CORPUS.length}-doc corpus (~${FULL_CORPUS_TOKENS} tokens) + tools`);
console.log(`  prefix (system + ${TOOL_SCHEMA.length} tools): ${PREFIX_TOKENS} tokens; top-k = ${TOP_K} per turn`);
console.log("");
console.log(`  ${pad("turn", 4)} | ${pad("A: monolithic", 14)} | ${pad("B: governed", 12)} | ${pad("turn saving", 12)}`);
console.log(`  ${"-".repeat(4)}-+-${"-".repeat(14)}-+-${"-".repeat(12)}-+-${"-".repeat(12)}`);
for (const r of rows) {
  const saving = r.tokensA - r.tokensB;
  console.log(`  ${pad(r.turn, 4)} | ${pad(r.tokensA, 14)} | ${pad(r.tokensB, 12)} | ${pad(saving, 12)}`);
}
console.log(`  ${"-".repeat(4)}-+-${"-".repeat(14)}-+-${"-".repeat(12)}-+-${"-".repeat(12)}`);
console.log(`  ${pad("tot", 4)} | ${pad(totalA, 14)} | ${pad(totalB, 12)} | ${pad(totalA - totalB, 12)}`);
console.log("");
console.log(`  TOTAL A (monolithic):   ${pad(totalA, 8)} input tokens`);
console.log(`  TOTAL B (governed RAG): ${pad(totalB, 8)} input tokens`);
console.log(`  REDUCTION:              ${reductionPct.toFixed(1)}%  (B uses ${(totalB / totalA).toFixed(2)}x of A)`);
console.log("");

// ── Output: where B saves ──────────────────────────────────────────────────────
console.log("  Where B saves (vs re-sending everything uncached every turn):");
const bucket = (label: string, n: number): string =>
  `    ${label.padEnd(34)} ${pad(n, 8)} tok  (${((n / savingsBucketTotal) * 100).toFixed(0)}%)`;
console.log(bucket("prompt-cache on system+tools prefix", savedByCache) + "   ← gateway cache_control");
console.log(bucket("retrieval scoping vs full corpus", savedByRetrieval) + "   ← rag-pipeline top-k");
console.log(bucket("working-memory history compression", savedByCompression) + "   ← keep floor(len/2)+summary");
console.log(bucket("ReAct observation masking", savedByMasking) + "   ← mask obs after N turns");
console.log("");

// ── Output: illustrative cost (clearly labeled representative pricing) ─────────
console.log("  Illustrative input cost @ representative Opus-tier pricing");
console.log(`  ($${OPUS_INPUT_USD_PER_MTOK.toFixed(2)}/1M input; cache reads ~${CACHE_READ_MULTIPLIER}x — VERIFY AGAINST CURRENT PRICING):`);
console.log(`    A (monolithic):   $${usd(totalA).toFixed(4)}`);
console.log(`    B (governed RAG): $${usd(totalB).toFixed(4)}`);
console.log(`    saving:           $${(usd(totalA) - usd(totalB)).toFixed(4)} over ${TURNS} turns`);
console.log("");

// ── Caveat: estimator + when B does NOT win ────────────────────────────────────
console.log(
  "  Caveat: chars/4 is an estimate (a real tokenizer differs in absolute terms; the RATIO is the\n" +
    "  robust number). B does NOT win on tiny single-turn tasks, or on corpora small enough to fit\n" +
    "  cheaply in one prompt — there, retrieval + caching add orchestration overhead with little to save."
);
console.log("");

// ── Self-verification ──────────────────────────────────────────────────────────
assert(rows.length === TURNS, `simulated all ${TURNS} turns`);
assert(totalB < totalA, "governed RAG (B) costs fewer tokens than monolithic (A)");
assert(reductionPct > 50 && reductionPct < 99, "reduction lands in a sane range (50%..99%)");
assert(savedByCache > 0, "prompt caching contributes positive savings");
assert(savedByRetrieval > 0, "retrieval scoping contributes positive savings");
assert(savedByCompression > 0, "history compression contributes positive savings");
assert(savedByMasking > 0, "observation masking contributes positive savings");
// Turn 1 has no history and no prior observations, so B's only edge is retrieval scoping.
const firstRow = rows[0];
assert(firstRow !== undefined && firstRow.tokensB < firstRow.tokensA, "even turn 1 favors B via retrieval scoping");
// The reduction must widen over the session as caching + compression + masking compound.
const lastRow = rows[rows.length - 1];
assert(
  firstRow !== undefined &&
    lastRow !== undefined &&
    lastRow.tokensA - lastRow.tokensB > firstRow.tokensA - firstRow.tokensB,
  "per-turn savings grow as the session lengthens"
);

console.log(`All assertions passed — governed RAG cuts input tokens by ${reductionPct.toFixed(1)}% on this workload.`);
console.log("");
