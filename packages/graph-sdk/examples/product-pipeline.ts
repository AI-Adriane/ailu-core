/**
 * Capstone — a GOVERNED product pipeline, brief → ship, composed from the catalog
 * and run on the engine (Rust when the `@ailu/napi` addon is present, else the
 * TypeScript fallback — the same graph runs on either).
 *
 * ── WHAT THIS DEMONSTRATES ────────────────────────────────────────────────────
 * A single graph that exercises every layer of `@ailu/graph-sdk` at once:
 *   - a PURE component (`components.promptBuilder`) that runs natively on Rust,
 *   - a REAL connector (`semanticRetriever`) grounding the work in a seeded corpus,
 *   - AGENT NODES across three capability tiers (balanced / frontier / creative) —
 *     the {@link ModelPolicy} resolving each stage's concrete model from the
 *     providers actually available (Mistral-only env → the mistral column),
 *   - a HUMAN-APPROVAL GATE (`humanGate('ship-gate')`) — the governance core: the
 *     run SUSPENDS before anything ships and a human decides go/no-go, then a
 *     `resume(runId)` carries it through to launch copy.
 *
 * ── THE PIPELINE ──────────────────────────────────────────────────────────────
 *   clarify → research → design → mvp → security → [ship-gate] → ship
 *
 *   1. clarify  — promptBuilder: {{brief}} → a focused researchQuery (pure, on Rust).
 *   2. research — semanticRetriever over a seeded corpus, then a balanced agent
 *                 writes a grounded research summary.
 *   3. design   — balanced agent: a product design outline from brief + research.
 *   4. mvp      — balanced agent: an MVP feature plan from the design.
 *   5. security — frontier agent: a security/risk review of the MVP (the costly
 *                 stage uses the top tier — the policy made visible).
 *   6. ship-gate— humanGate: GOVERNANCE. The run suspends; a human approves go/no-go.
 *   7. ship     — creative agent: launch / changelog copy from the approved plan.
 *
 * ── OFFLINE vs LIVE (no key required to run) ──────────────────────────────────
 * Embeddings: {@link semanticRetriever} defaults to real Mistral embeddings. To stay
 * runnable WITHOUT a key AND live WITH one, this example reads MISTRAL_API_KEY:
 *   - no key  → inject a tiny DETERMINISTIC fake embedder (no network, never crashes),
 *               and run the agents on a deterministic MOCK gateway,
 *   - a key   → use the real {@link createEmbeddings}() and let the agents resolve
 *               their tier to a real Mistral model and make real (short) calls.
 *
 * Capability tiers: the per-stage tier is always RESOLVED and reported through the
 * {@link ModelPolicy} (`resolveAgentModel`) so you can see "balanced/frontier/creative
 * → <concrete model>" for the current env. Offline (mock gateway) the agent nodes run
 * tier-agnostic so the run completes deterministically on either engine; with a key
 * the tier is bound to the agent so the policy drives the real model choice.
 *
 * Run it:
 *   pnpm --filter @ailu/graph-sdk example:product
 *   pnpm --filter @ailu/graph-sdk exec node --import tsx examples/product-pipeline.ts
 */

import {
  components,
  createGraph,
  DefaultLLMGateway,
  MockLLMProviderAdapter,
  ModelPolicy,
  semanticRetriever,
  type AgentResult,
  type CompiledGraph,
  type Embeddings,
  type LLMGateway,
  type ModelTier,
  type RunId
} from "@ailu/graph-sdk";
// `resolveAgentModel` is the TS-side ModelPolicy resolver (mirrors the Rust
// `resolve_agent_model`); imported from source since it is an SDK-internal helper
// not re-exported on the package index.
import { resolveAgentModel } from "../src/agent-node.js";

// ── The five pipeline stages and their capability tiers ──────────────────────
// The costly security review uses the top (`frontier`) tier; the creative launch
// copy uses the `creative` tier; the rest are `balanced`. This is the policy the
// example makes visible per stage.
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
   * The LLM gateway every agent node runs on. Defaults to a deterministic MOCK gateway
   * (registered under the nominal provider) so the pipeline runs end-to-end with no
   * provider keys. Inject a real gateway to run the agents live.
   */
  llm?: LLMGateway;
  /**
   * The embeddings client backing the `research` retriever. Defaults to: a tiny
   * DETERMINISTIC fake when MISTRAL_API_KEY is absent (offline, never crashes), else
   * the real {@link createEmbeddings}() (live Mistral embeddings). Inject a fake to
   * force a test offline regardless of the env.
   */
  embeddings?: Embeddings;
  /**
   * Bind each agent node to its capability {@link ModelTier} so the {@link ModelPolicy}
   * resolves a concrete model per stage. Defaults to `true` only when a real provider
   * key is present (so the policy drives a live model); offline it defaults to `false`
   * so the agent nodes run tier-agnostic on the deterministic mock and the run
   * completes the same way on either engine. The intended tiers are reported either
   * way (see {@link resolvedStageModels}).
   */
  bindTiers?: boolean;
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

/** Detect a usable Mistral key without importing any secret material. */
const hasMistralKey = (): boolean => {
  const key = process.env.MISTRAL_API_KEY;
  return key !== undefined && key.length > 0;
};

/**
 * A tiny DETERMINISTIC fake embedder: a 4-bucket character-count vector per text, so
 * cosine ranking is stable and the example never touches the network offline. (The
 * same shape the engine's mock RAG embedder uses.)
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
 * The default deterministic MOCK gateway: every agent turn returns a final answer, so
 * each agent node completes and writes a non-empty {@link AgentResult} with no keys.
 * Registered under the nominal `anthropic` provider slot the agent nodes target offline.
 */
const mockGateway = (): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new MockLLMProviderAdapter({
      provider: "anthropic",
      response: {
        content:
          "FINAL: grounded, governance-first plan — checkpointed, auditable, and gated " +
          "before ship.",
        usage: { promptTokens: 0, completionTokens: 0 },
        model: "mock",
        provider: "anthropic"
      }
    })
  );
  return gateway;
};

/**
 * Resolve, per stage, the concrete `{ provider, model }` the {@link ModelPolicy} would
 * pick for that tier in the CURRENT env — the policy made visible. Offline (no key) the
 * mock-bound stages resolve through the explicit `anthropic` slot so the mapping is
 * concrete and reportable; with a key they resolve to the live (Mistral) column.
 */
export const resolvedStageModels = (
  live: boolean
): Record<keyof StageTiers, { tier: ModelTier; provider?: string; model?: string }> => {
  const provider = live ? undefined : ("anthropic" as const);
  const entries = Object.entries(STAGE_TIERS) as [keyof StageTiers, ModelTier][];
  const out = {} as Record<keyof StageTiers, { tier: ModelTier; provider?: string; model?: string }>;
  for (const [stage, tier] of entries) {
    const resolved = resolveAgentModel({ tier, provider });
    out[stage] = { tier, provider: resolved.provider, model: resolved.model };
  }
  return out;
};

/**
 * Build the governed product pipeline as a typed, runnable {@link CompiledGraph}.
 *
 * The graph: `clarify → research → design → mvp → security → ship-gate → ship`. The
 * `ship-gate` human gate suspends the run before any launch copy is written; a
 * `resume(runId)` (after a human's go decision) drives it through `ship`.
 */
export const buildProductPipeline = (
  options: ProductPipelineOptions = {}
): CompiledGraph<ProductPipelineChannels> => {
  const live = hasMistralKey();
  const llm = options.llm ?? mockGateway();
  const embeddings = options.embeddings ?? (live ? undefined : fakeEmbeddings());
  // Bind tiers to the agent nodes only when we have a live provider (so the policy
  // drives a real model). Offline we run tier-agnostic on the mock so the run
  // completes deterministically on either engine; the intended tiers are still
  // reported via `resolvedStageModels`.
  const bindTiers = options.bindTiers ?? live;

  // Only carry the agent-node tier/provider when binding tiers; offline the mock
  // gateway answers on its nominal `anthropic` slot with no tier resolution.
  const agentTier = (stage: keyof StageTiers): { tier?: ModelTier; provider?: "mistral" } =>
    bindTiers ? { tier: STAGE_TIERS[stage], provider: live ? "mistral" : undefined } : {};

  // A short prompt per agent keeps live token use modest. The agent sees the full
  // channel map in its first user turn, so it grounds on the upstream stages.
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
    // 2a. research retrieval — a real connector grounding the work in the corpus.
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
    // 2b. research summary — balanced agent writes a grounded summary.
    .agentNode("research", {
      llm,
      prompt: {
        system:
          "You are a market researcher. Using the retrieved notes in state, write a short " +
          "research summary grounding the product. Be concise."
      },
      outputChannel: "research",
      maxIterations: 2,
      ...agentTier("research")
    })
    // 3. design — balanced agent: a design outline from brief + research.
    .agentNode("design", {
      llm,
      prompt: {
        system:
          "You are a product designer. From the brief and research in state, write a short " +
          "product design outline. Be concise."
      },
      outputChannel: "design",
      maxIterations: 2,
      ...agentTier("design")
    })
    // 4. mvp — balanced agent: an MVP feature plan from the design.
    .agentNode("mvp", {
      llm,
      prompt: {
        system:
          "You are a delivery lead. From the design in state, write a short MVP feature plan " +
          "(the smallest shippable slice). Be concise."
      },
      outputChannel: "mvpPlan",
      maxIterations: 2,
      ...agentTier("mvp")
    })
    // 5. security — FRONTIER agent: the costly stage uses the top tier.
    .agentNode("security", {
      llm,
      prompt: {
        system:
          "You are a security reviewer. From the MVP plan in state, write a short security and " +
          "risk review, flagging anything that must be gated before ship. Be concise."
      },
      outputChannel: "securityReview",
      maxIterations: 2,
      ...agentTier("security")
    })
    // 6. ship-gate — GOVERNANCE: the run suspends here for a human go/no-go.
    .humanGate("ship-gate")
    // 7. ship — CREATIVE agent: launch / changelog copy from the approved plan.
    .agentNode("ship", {
      llm,
      prompt: {
        system:
          "You are a launch copywriter. From the approved plan in state, write short launch / " +
          "changelog copy. Be punchy and concise."
      },
      outputChannel: "shipCopy",
      maxIterations: 2,
      ...agentTier("ship")
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

/** First non-empty `FINAL:`/`final:` line of an agent's reasoning, else the raw text. */
const summarize = (result: AgentResult | null | undefined): string => {
  if (result === null || result === undefined) {
    return "(no output)";
  }
  const reasoning = result.reasoning ?? "";
  const final = /final:\s*(.+)$/im.exec(reasoning)?.[1];
  const text = (final ?? reasoning).trim();
  return text.length > 0 ? text : "(empty)";
};

const isMain = (): boolean => {
  const entry = process.argv[1];
  return entry !== undefined && import.meta.url === `file://${entry}`;
};

const main = async (): Promise<void> => {
  const live = hasMistralKey();

  const app = buildProductPipeline();
  const engine = app.usesRustEngine ? "Rust (@ailu/napi)" : "TypeScript (fallback)";

  // Lifecycle journal — every node transition emits an event (the governance audit trail).
  const journal: string[] = [];
  app.onEvent((event) => {
    const node = "nodeId" in event ? `:${String(event.nodeId)}` : "";
    journal.push(`${event.type}${node}`);
  });

  console.log("");
  console.log("Governed product pipeline — brief → ship");
  console.log(`  engine: ${engine}`);
  console.log(`  mode:   ${live ? "LIVE (MISTRAL_API_KEY present)" : "OFFLINE (deterministic mock + fake embeddings)"}`);

  // The model policy, made visible per stage.
  console.log("");
  console.log("  Capability tiers resolved by the ModelPolicy for this env:");
  const models = resolvedStageModels(live);
  for (const [stage, info] of Object.entries(models)) {
    const provider = info.provider ?? "(default)";
    const model = info.model ?? "(react-agent default)";
    console.log(`    ${stage.padEnd(9)} tier=${info.tier.padEnd(9)} → ${provider} / ${model}`);
  }

  const RUN_ID = "run_product_pipeline_demo" as RunId;
  const brief =
    "A governance studio for teams running fleets of AI agents: approve, audit, and resume " +
    "every run.";

  // ── Act 1: run until the ship-gate human gate ──────────────────────────────
  const atGate = await app.run({ brief }, { runId: RUN_ID });

  if (atGate.status === "suspended") {
    console.log("");
    console.log("Pipeline state so far (suspended at the governance gate):");
    console.log(`  researchQuery: ${atGate.channels.researchQuery}`);
    console.log(`  research:      ${summarize(atGate.channels.research)}`);
    console.log(`  design:        ${summarize(atGate.channels.design)}`);
    console.log(`  mvp:           ${summarize(atGate.channels.mvpPlan)}`);
    console.log(`  security:      ${summarize(atGate.channels.securityReview)}`);
    console.log("");
    console.log(`GOVERNANCE: awaiting human go/no-go (suspended at '${String(atGate.currentNodeId)}')`);

    // ── Act 2: a human approves go; resume drives the run to ship ─────────────
    console.log("");
    console.log("Human decision: GO. Resuming the run to write launch copy…");
    const shipped = await app.resume(RUN_ID);

    console.log("");
    console.log(`Final status: ${shipped.status}`);
    console.log(`Launch copy (shipCopy):`);
    console.log(`  ${summarize(shipped.channels.shipCopy)}`);
  } else {
    // Defensive: the gate should always suspend; report if it didn't.
    console.log("");
    console.log(`Unexpected: the run did not suspend at the gate (status=${atGate.status}).`);
  }

  console.log("");
  console.log("Lifecycle journal (every node transition emits an event):");
  for (const entry of journal) {
    console.log(`  ${entry}`);
  }
  console.log("");

  // A light, honest note when running offline.
  if (!live) {
    console.log(
      "  Note: offline mode used a deterministic mock gateway + a fake embedder, so the " +
        "agent text is illustrative. Set MISTRAL_API_KEY to run the agents and embeddings live."
    );
    console.log("");
  }

  // Demonstrate the policy is consistent with the engine's own resolver.
  void new ModelPolicy();
};

if (isMain()) {
  await main();
}
