import { afterEach, beforeEach, describe, expect, it } from "vitest";

// `@ailu-ai/graph-sdk` is aliased to source in vitest.config.ts (same module instance
// the example uses), so the test exercises current source, not a stale `dist/`.
import { rustEngineAvailable, type AgentResult, type Embeddings, type RunId } from "@ailu-ai/graph-sdk";

import { buildProductPipeline, resolvedStageModels } from "./product-pipeline.js";

/**
 * The capstone governed product pipeline, exercised end-to-end with NO provider keys: an
 * injected DETERMINISTIC fake embedder, and the agents on the engine's offline mock
 * (`AILU_LLM_MOCK=1`, set by vitest.config.ts). It proves the governance core — the run
 * SUSPENDS at the `ship-gate` human gate, and a `resume` drives it to `completed` with a
 * non-empty `shipCopy`. Only structure is asserted, never the agents' wording.
 */

/** A tiny deterministic fake embedder (fixed vectors; no network). */
const fakeEmbeddings: Embeddings = {
  embed: (texts) => Promise.resolve(texts.map((_text, index) => [index + 1, 1, 0, 0]))
};

const isNonEmptyAgentResult = (value: AgentResult | null | undefined): boolean =>
  value !== null &&
  value !== undefined &&
  typeof value.reasoning === "string" &&
  value.reasoning.trim().length > 0;

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu-ai/graph-sdk — capstone product pipeline (governed, offline)", () => {
  // Force every provider key off so the agents run on the deterministic offline mock
  // (no live calls, no key-dependent tier resolution).
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

  it("suspends at the ship-gate, then resume drives it to completed with non-empty shipCopy", async () => {
    const app = buildProductPipeline({ embeddings: fakeEmbeddings });

    const runId = "run_product_pipeline_test" as RunId;
    const brief =
      "A governance studio for teams running fleets of AI agents: approve, audit, resume.";

    // Act 1 — run until the governance gate.
    const atGate = await app.run({ brief }, { runId });

    // GOVERNANCE: the run suspends cleanly at the human-approval gate before shipping.
    expect(atGate.status).toBe("suspended");
    expect(String(atGate.currentNodeId)).toBe("ship-gate");
    // The earlier stages ran and grounded the work: the clarify component rendered the
    // research query, and each upstream agent wrote a non-empty result.
    expect(atGate.channels.researchQuery).toContain(brief);
    expect(isNonEmptyAgentResult(atGate.channels.research)).toBe(true);
    expect(isNonEmptyAgentResult(atGate.channels.design)).toBe(true);
    expect(isNonEmptyAgentResult(atGate.channels.mvpPlan)).toBe(true);
    expect(isNonEmptyAgentResult(atGate.channels.securityReview)).toBe(true);
    // Nothing has shipped yet — the gate is the seam before the creative ship stage.
    expect(isNonEmptyAgentResult(atGate.channels.shipCopy)).toBe(false);

    // Act 2 — a human approves go; resume past the gate to the ship stage.
    const shipped = await app.resume(runId);

    expect(shipped.status).toBe("completed");
    // The governed run produced launch copy after the gate.
    expect(isNonEmptyAgentResult(shipped.channels.shipCopy)).toBe(true);
  });

  it("reports the per-stage capability tiers through the ModelPolicy", () => {
    // With no provider key the policy reports the offline mock for every tier; the tier per
    // stage shows the costly security stage uses `frontier` and ship uses `creative`.
    const models = resolvedStageModels({});
    expect(models.research.tier).toBe("balanced");
    expect(models.design.tier).toBe("balanced");
    expect(models.mvp.tier).toBe("balanced");
    expect(models.security.tier).toBe("frontier");
    expect(models.ship.tier).toBe("creative");
    for (const info of Object.values(models)) {
      expect(info.model.length).toBeGreaterThan(0);
    }

    // With a Mistral key only, every tier resolves to the mistral column.
    const mistralOnly = resolvedStageModels({ MISTRAL_API_KEY: "test-key" });
    for (const info of Object.values(mistralOnly)) {
      expect(info.provider).toBe("mistral");
    }
    expect(mistralOnly.security.model).toBe("mistral-large-latest");
  });

  it("compiles the full brief→ship pipeline with the ship-gate human gate", () => {
    const app = buildProductPipeline({ embeddings: fakeEmbeddings });
    const nodeIds = app.definition.nodes.map((node) => String(node.id));
    // The ordered stages plus the governance gate are all present.
    expect(nodeIds).toEqual([
      "clarify",
      "retrieve",
      "research",
      "design",
      "mvp",
      "security",
      "ship-gate",
      "ship"
    ]);
    // The ship-gate is a human-gate node (the suspend seam), and the pipeline enters at clarify.
    const gate = app.definition.nodes.find((node) => String(node.id) === "ship-gate");
    expect(gate?.type).toBe("human-gate");
    expect(String(app.definition.entryNodeId)).toBe("clarify");
    // Each agent stage asks for a capability tier, carried on its node for the engine.
    const security = app.definition.nodes.find((node) => String(node.id) === "security");
    expect((security?.metadata?.agent as { tier?: string } | undefined)?.tier).toBe("frontier");
    expect(app.usesRustEngine).toBe(true);
  });
});
