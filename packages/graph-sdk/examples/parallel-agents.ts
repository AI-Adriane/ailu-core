/**
 * Run agents in parallel, two ways.
 *
 * 1. `mapAgents`: one sub-agent per item of a list, all running concurrently. The results land
 *    in input order, whatever order the agents finish in, so the run is deterministic and
 *    resumable.
 * 2. `council`: several member agents answer the same question, reviewers rank the answers
 *    without knowing who wrote them, and a chair writes the final answer from the ranking.
 *    `council()` returns a graph definition, which you run with `runCatalogGraph`.
 *
 * Run it offline: AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/parallel-agents.ts
 * With ANTHROPIC_API_KEY set, every agent runs on Claude.
 */
import { council, createGraph, finalAnswer, model, runCatalogGraph, type AgentResult } from "@ailu-ai/graph-sdk";

const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

const sonnet = model.anthropic("claude-sonnet-4-6");

// ── 1. mapAgents: summarize every review concurrently ────────────────────────
const REVIEWS = [
  "Setup took five minutes and the docs answered every question I had.",
  "Great product, but the invoice PDF is missing our VAT number.",
  "Support replied within an hour and fixed the sync bug the same day."
];

const app = createGraph({ name: "review-summaries" })
  .channel("reviews", { type: "json", default: [] as string[] })
  .mapAgents("summarize", {
    overChannel: "reviews", // one sub-agent per item of this list
    subAgent: { model: sonnet, prompt: { system: "Summarize the customer review in five words." } },
    joinAt: "summaries" // AgentResult[], in the same order as `reviews`
  })
  .compile();

console.log("\n1. mapAgents");
const out = await app.run({ reviews: REVIEWS });
check(out.status === "completed", "the fan-out completed");
check(out.channels.summaries.length === REVIEWS.length, "one result per review, in input order");
check(out.channels.summaries.every((result) => finalAnswer(result).length > 0), "every sub-agent answered");
out.channels.summaries.forEach((result, index) => console.log(`  ${index + 1}. ${finalAnswer(result)}`));

// ── 2. council: three perspectives, anonymous peer review, one chair ─────────
const seat = (perspective: string) => ({ model: sonnet, prompt: { system: `Answer as ${perspective}.` } });

const definition = council({
  members: [seat("a pragmatic staff engineer"), seat("a security reviewer"), seat("a cost-conscious CTO")],
  // reviewers default to one per member; each ranks the anonymized answers A, B, C
  chair: { model: model.anthropic.frontier, prompt: { system: "Synthesize the best final answer." } }
});

console.log("\n2. council");
const outcome = await runCatalogGraph(definition, {
  initialData: { query: "Postgres or SQLite for a three-person startup's first product?" }
});
const ranking = outcome.state.channels.aggregate as string[]; // consensus, best first
const answer = finalAnswer(outcome.state.channels.answer as AgentResult | undefined);

check(outcome.status === "completed", "the council completed");
check([...ranking].sort().join() === "A,B,C", "the consensus ranks all three anonymized answers");
check(answer.length > 0, "the chair wrote the final answer");
console.log(`  Ranking: ${ranking.join(" > ")}`);
console.log(`  Answer:  ${answer}`);
