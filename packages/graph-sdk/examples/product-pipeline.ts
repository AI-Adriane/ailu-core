/**
 * Capstone — a GOVERNED product pipeline, brief → ship, composed from the catalog and run on
 * the Rust engine.
 *
 * ── WHAT THIS DEMONSTRATES ────────────────────────────────────────────────────
 * A single graph that exercises every layer of `@ailu-ai/graph-sdk` at once:
 *   - a PURE component (`components.promptBuilder`) that runs natively on Rust,
 *   - a REAL connector (`semanticRetriever`) grounding the work in a seeded corpus,
 *   - AGENT NODES across three capability tiers (balanced / frontier / creative). Each stage
 *     asks for a tier, not a model (`model.balanced`, `model.frontier`, `model.creative`);
 *     the engine's ModelPolicy picks the concrete model from the providers whose keys are
 *     set (Mistral-only env → the mistral column),
 *   - a HUMAN-APPROVAL GATE (`humanGate('ship-gate')`) — the governance core: the run
 *     SUSPENDS before anything ships and a human decides go/no-go, then a `resume(runId)`
 *     carries it through to launch copy.
 *
 * ── THE PIPELINE ──────────────────────────────────────────────────────────────
 *   clarify → retrieve → research → design → mvp → security → [ship-gate] → ship
 *
 *   1. clarify   — promptBuilder: {{brief}} → a focused researchQuery (pure, on Rust).
 *   2. retrieve  — semanticRetriever over a seeded corpus.
 *   3. research  — balanced agent: a grounded research summary.
 *   4. design    — balanced agent: a product design outline from brief + research.
 *   5. mvp       — balanced agent: an MVP feature plan from the design.
 *   6. security  — frontier agent: a security/risk review of the MVP (the costly stage uses
 *                  the top tier — the policy made visible).
 *   7. ship-gate — humanGate: GOVERNANCE. The run suspends; a human approves go/no-go.
 *   8. ship      — creative agent: launch / changelog copy from the approved plan.
 *
 * ── OFFLINE vs LIVE ───────────────────────────────────────────────────────────
 * Embeddings: {@link semanticRetriever} defaults to real Mistral embeddings. To stay runnable
 * WITHOUT a key AND live WITH one, this example reads MISTRAL_API_KEY:
 *   - no key  → inject a tiny DETERMINISTIC fake embedder (no network, never crashes), and
 *               run the agents on the engine's offline mock (set AILU_LLM_MOCK=1),
 *   - a key   → use the real {@link createEmbeddings}() and let every tier resolve to a real
 *               Mistral model.
 *
 * Run it:
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:product
 */

import {
  components,
  createGraph,
  finalAnswer,
  model,
  ModelPolicy,
  semanticRetriever,
  type AgentResult,
  type CompiledGraph,
  type Embeddings,
  type ModelTier,
  type RunId
} from "@ailu-ai/graph-sdk";

// ── The five agent stages and their capability tiers ─────────────────────────
// The costly security review uses the top (`frontier`) tier; the creative launch copy uses
// the `creative` tier; the rest are `balanced`. This is the policy the example makes visible.
export type StageTiers = {
  research: ModelTier;
  design: ModelTier;
  mvp: ModelTier;
  security: ModelTier;
  ship: ModelTier;
};

const STAGE_TIERS: StageTiers = {
  research: "balanced",
  design: "balanced",
  mvp: "balanced",
  security: "frontier",
  ship: "creative"
};

// ── The typed channels the pipeline state flows through ──────────────────────
/** The channels a {@link buildProductPipeline} graph exposes (each stage's output). */
export type ProductPipelineChannels = {
  brief: string;
  researchQuery: string;
  research: AgentResult;
  design: AgentResult;
  mvpPlan: AgentResult;
  securityReview: AgentResult;
  shipCopy: AgentResult;
};

/** Options for {@link buildProductPipeline}. */
export type ProductPipelineOptions = {
  /**
   * The embeddings client backing the `retrieve` step. Defaults to: a tiny DETERMINISTIC
   * fake when MISTRAL_API_KEY is absent (offline, never crashes), else the real
   * {@link createEmbeddings}() (live Mistral embeddings). Inject a fake to force a test
   * offline regardless of the env.
   */
  embeddings?: Embeddings;
};

/** A short seeded corpus of market / best-practice notes that grounds the research. */
const CORPUS: { id: string; content: string }[] = [
  {
    id: "governance",
    content:
      "Buyers of agent platforms rank governance first: every sensitive action must be " +
      "checkpointed, attributable, and pass a human-approval gate before it ships."
  },
  {
    id: "resumability",
    content:
      "A resumable runtime that survives crashes and suspends cleanly for approval is a " +
      "top differentiator; teams abandon tools that lose work on failure."
  },
  {
    id: "observability",
    content:
      "Operators expect a full event journal — every node lifecycle transition emitted — " +
      "so a run can be audited and replayed long after it completed."
  },
  {
    id: "cost-control",
    content:
      "Cost control wins deals: route cheap stages to small models and reserve a frontier " +
      "model for the few high-stakes steps, with prompt caching on the stable prefix."
  },
  {
    id: "time-to-value",
    content:
      "Fast time-to-value matters: a prebuilt component catalog and a fluent builder let a " +
      "team ship a governed pipeline in an afternoon instead of a quarter."
  }
];

/** Detect a usable Mistral key without reading any secret material. */
const hasMistralKey = (): boolean => {
  const key = process.env.MISTRAL_API_KEY;
  return key !== undefined && key.length > 0;
};

/**
 * A tiny DETERMINISTIC fake embedder: a 4-bucket character-count vector per text, so cosine
 * ranking is stable and the example never touches the network offline.
 */
const fakeEmbeddings = (): Embeddings => ({
  embed: (texts) =>
    Promise.resolve(
      texts.map((text) => {
        const counts = [0, 0, 0, 0];
        for (const char of text) {
          const idx = (char.codePointAt(0) ?? 0) % counts.length;
          counts[idx] = (counts[idx] ?? 0) + 1;
        }
        return counts;
      })
    )
});

/**
 * Resolve, per stage, the concrete `{ provider, model }` the {@link ModelPolicy} picks for that
 * tier from the keys in `env` — the policy made visible. With no key it reports the offline
 * mock. (The SDK's policy table covers Anthropic, Mistral and Ollama.)
 */
export const resolvedStageModels = (
  env: NodeJS.ProcessEnv = process.env
): Record<keyof StageTiers, { tier: ModelTier; provider: string; model: string }> => {
  const policy = new ModelPolicy();
  const available = policy.availableFromEnv(env);
  const entries = Object.entries(STAGE_TIERS) as [keyof StageTiers, ModelTier][];
  const out = {} as Record<keyof StageTiers, { tier: ModelTier; provider: string; model: string }>;
  for (const [stage, tier] of entries) {
    const choice = policy.resolve(tier, available);
    out[stage] = { tier, provider: choice.provider, model: choice.model };
  }
  return out;
};

/**
 * Build the governed product pipeline as a typed, runnable {@link CompiledGraph}.
 *
 * The graph: `clarify → retrieve → research → design → mvp → security → ship-gate → ship`. The
 * `ship-gate` human gate suspends the run before any launch copy is written; a
 * `resume(runId)` (after a human's go decision) drives it through `ship`.
 */
export const buildProductPipeline = (
  options: ProductPipelineOptions = {}
): CompiledGraph<ProductPipelineChannels> => {
  const embeddings = options.embeddings ?? (hasMistralKey() ? undefined : fakeEmbeddings());

  // Each agent asks for a capability tier, not a model (`model.balanced`, `model.frontier`, …):
  // the engine resolves the concrete model from the keys in the env.
  const tierModel = (stage: keyof StageTiers) => model[STAGE_TIERS[stage]];

  // Short prompts keep live token use modest. Each agent sees the channel map in its first
  // user turn, so it grounds on the upstream stages.
  return createGraph({ name: "product-pipeline" })
    .channel("brief", { type: "string", default: "" })
    .channel("researchQuery", { type: "string", default: "" })
    .channel("retrieved", { type: "json", default: [] })
    // 1. clarify — pure component, runs natively on Rust.
    .component(
      "clarify",
      components.promptBuilder({
        template:
          "Research the market and best practices most relevant to this product brief, " +
          "in one focused question: {{brief}}",
        into: "researchQuery"
      })
    )
    // 2. retrieve — a real connector grounding the work in the corpus.
    .node(
      "retrieve",
      semanticRetriever({
        queryFrom: "researchQuery",
        into: "retrieved",
        k: 3,
        docs: CORPUS,
        ...(embeddings === undefined ? {} : { embeddings })
      })
    )
    // 3. research — balanced agent writes a grounded summary.
    .agentNode("research", {
      model: tierModel("research"),
      prompt: {
        system:
          "You are a market researcher. Using the retrieved notes in state, write a short " +
          "research summary grounding the product. Be concise."
      },
      outputChannel: "research",
      maxIterations: 2
    })
    // 4. design — balanced agent: a design outline from brief + research.
    .agentNode("design", {
      model: tierModel("design"),
      prompt: {
        system:
          "You are a product designer. From the brief and research in state, write a short " +
          "product design outline. Be concise."
      },
      outputChannel: "design",
      maxIterations: 2
    })
    // 5. mvp — balanced agent: an MVP feature plan from the design.
    .agentNode("mvp", {
      model: tierModel("mvp"),
      prompt: {
        system:
          "You are a delivery lead. From the design in state, write a short MVP feature plan " +
          "(the smallest shippable slice). Be concise."
      },
      outputChannel: "mvpPlan",
      maxIterations: 2
    })
    // 6. security — FRONTIER agent: the costly stage uses the top tier.
    .agentNode("security", {
      model: tierModel("security"),
      prompt: {
        system:
          "You are a security reviewer. From the MVP plan in state, write a short security and " +
          "risk review, flagging anything that must be gated before ship. Be concise."
      },
      outputChannel: "securityReview",
      maxIterations: 2
    })
    // 7. ship-gate — GOVERNANCE: the run suspends here for a human go/no-go.
    .humanGate("ship-gate")
    // 8. ship — CREATIVE agent: launch / changelog copy from the approved plan.
    .agentNode("ship", {
      model: tierModel("ship"),
      prompt: {
        system:
          "You are a launch copywriter. From the approved plan in state, write short launch / " +
          "changelog copy. Be punchy and concise."
      },
      outputChannel: "shipCopy",
      maxIterations: 2
    })
    .edge("clarify", "retrieve")
    .edge("retrieve", "research")
    .edge("research", "design")
    .edge("design", "mvp")
    .edge("mvp", "security")
    .edge("security", "ship-gate")
    .edge("ship-gate", "ship")
    .entry("clarify")
    .compile() as CompiledGraph<ProductPipelineChannels>;
};

// ── Runnable main (executed when this file is run directly) ───────────────────

/** An agent stage's final answer, or a placeholder when it wrote nothing. */
const summarize = (result: AgentResult | null | undefined): string =>
  finalAnswer(result ?? undefined) || "(no output)";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

/** Build the pipeline, run it to the ship-gate, approve, and resume it to completion. */
export const main = async (): Promise<void> => {
  const live = hasMistralKey();
  const app = buildProductPipeline();

  // Lifecycle journal — every node transition emits an event (the governance audit trail).
  const journal: string[] = [];
  app.onEvent((event) => {
    journal.push("nodeId" in event ? `${event.type}:${String(event.nodeId)}` : event.type);
  });

  console.log("");
  console.log("Governed product pipeline — brief → ship");
  console.log(`  mode: ${live ? "LIVE (MISTRAL_API_KEY present)" : "OFFLINE (engine mock + fake embeddings)"}`);

  // The model policy, made visible per stage.
  console.log("");
  console.log("  Capability tiers resolved by the ModelPolicy for this env:");
  for (const [stage, info] of Object.entries(resolvedStageModels())) {
    console.log(`    ${stage.padEnd(9)} tier=${info.tier.padEnd(9)} → ${info.provider} / ${info.model}`);
  }

  const RUN_ID = "run_product_pipeline_demo" as RunId;
  const brief =
    "A governance studio for teams running fleets of AI agents: approve, audit, and resume " +
    "every run.";

  // ── Act 1: run until the ship-gate human gate ──────────────────────────────
  const atGate = await app.run({ brief }, { runId: RUN_ID });

  console.log("");
  check(atGate.status === "suspended", "the run suspended before shipping");
  check(String(atGate.currentNodeId) === "ship-gate", "it is paused at the ship-gate human gate");
  check(atGate.channels.researchQuery.includes(brief), "the clarify component rendered the research query");
  check(finalAnswer(atGate.channels.securityReview).length > 0, "the security review was written");

  console.log("");
  console.log("Pipeline state so far (suspended at the governance gate):");
  console.log(`  researchQuery: ${atGate.channels.researchQuery}`);
  console.log(`  research:      ${summarize(atGate.channels.research)}`);
  console.log(`  design:        ${summarize(atGate.channels.design)}`);
  console.log(`  mvp:           ${summarize(atGate.channels.mvpPlan)}`);
  console.log(`  security:      ${summarize(atGate.channels.securityReview)}`);

  // ── Act 2: a human approves go; resume drives the run to ship ─────────────
  console.log("");
  console.log("Human decision: GO. Resuming the run to write launch copy…");
  const shipped = await app.resume(RUN_ID);

  check(shipped.status === "completed", "the run completed after the go decision");
  check(finalAnswer(shipped.channels.shipCopy).length > 0, "the launch copy was written after the gate");
  console.log(`  Launch copy: ${summarize(shipped.channels.shipCopy)}`);

  console.log("");
  console.log("Lifecycle journal (every node transition emits an event):");
  for (const entry of journal) {
    console.log(`  ${entry}`);
  }
  console.log("");
};

const isMain = (): boolean => {
  const entry = process.argv[1];
  return entry !== undefined && import.meta.url === `file://${entry}`;
};

if (isMain()) {
  await main();
}
