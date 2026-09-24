import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { prebuilt } from "./index.js";
import type { AgentResult } from "./index.js";

/**
 * Prebuilt micro-agent tests. Each prebuilt graph runs on a deterministic mock
 * gateway by default, so these run with no provider keys and no network. We force the
 * provider keys off so resolution is reproducible regardless of the developer's env.
 */
describe("@ailu-ai/graph-sdk — prebuilt micro-agents", () => {
  const PROVIDER_KEYS = ["ANTHROPIC_API_KEY", "MISTRAL_API_KEY", "AILU_USE_OLLAMA"] as const;
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    // Run on the Rust engine (the only engine — the TS fallback was removed): these
    // assert the mock-gateway agent run completes, which is engine-independent in structure.
    saved.AILU_SDK_ENGINE = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = "rust";
    for (const key of PROVIDER_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(() => {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  });

  it("summarizer() compiles and runs to completion on the mock", async () => {
    const app = prebuilt.summarizer();

    const result = await app.run({ question: "A long piece of text to condense." });
    expect(result.status).toBe("completed");
    // Its result lands in the agent's `summary` output channel.
    const agentResult = (result.channels as Record<string, AgentResult>).summary;
    expect(agentResult).toBeDefined();
  });

  it("classifier() and extractor() also compile and run to completion", async () => {
    const classified = await prebuilt.classifier().run({ question: "I love this!" });
    expect(classified.status).toBe("completed");
    expect((classified.channels as Record<string, AgentResult>).label).toBeDefined();

    const extracted = await prebuilt.extractor().run({ question: "Ada at acme, ada@acme.io" });
    expect(extracted.status).toBe("completed");
    expect((extracted.channels as Record<string, AgentResult>).extracted).toBeDefined();
  });

  it("refundApprover() suspends for human approval before the refund tool runs", async () => {
    const app = prebuilt.refundApprover();

    const suspended = await app.run({ question: "Please refund order 42." }, { runId: "run_refund" as never });
    // Its tool is `requiresApproval` and the agent suspends for approval.
    expect(suspended.status).toBe("suspended");
  });

  it("ragAnswerer() composes retriever + reranker components with an agent step", async () => {
    const app = prebuilt.ragAnswerer({
      docs: [
        { id: "d1", content: "Ailu checkpoints after every node." },
        { id: "d2", content: "Unrelated content about the weather." }
      ]
    });

    const result = await app.run({ question: "When does Ailu checkpoint?" });
    expect(result.status).toBe("completed");
    const channels = result.channels as Record<string, unknown>;
    // The retriever + reranker populated their channels, and the agent answered.
    expect(Array.isArray(channels.retrieved)).toBe(true);
    expect(Array.isArray(channels.ranked)).toBe(true);
    expect(channels.answer).toBeDefined();
  });

  it("accepts a tierOverride and a pinned model", () => {
    // Compiles cleanly with light options; behaviour is covered by the run tests.
    const app = prebuilt.summarizer({ tierOverride: "balanced", model: "pinned" });
    expect(app.definition.name).toBe("prebuilt-summarizer");
  });

  it("translator() (fast) compiles and runs to completion on the mock", async () => {
    const app = prebuilt.translator();
    expect(app.definition.name).toBe("prebuilt-translator");

    const result = await app.run({ question: "Translate 'hello' to French." });
    expect(result.status).toBe("completed");
    // Its result lands in the `translation` output channel.
    expect((result.channels as Record<string, AgentResult>).translation).toBeDefined();
  });

  it("codeReviewer() (frontier) compiles and runs to completion on the mock", async () => {
    const app = prebuilt.codeReviewer();
    expect(app.definition.name).toBe("prebuilt-codeReviewer");

    const result = await app.run({ question: "function add(a,b){return a-b}" });
    expect(result.status).toBe("completed");
    expect((result.channels as Record<string, AgentResult>).review).toBeDefined();
  });

  it("copyEditor() (creative) compiles and runs to completion on the mock", async () => {
    const app = prebuilt.copyEditor();
    expect(app.definition.name).toBe("prebuilt-copyEditor");

    const result = await app.run({ question: "this sentance has a typo and is to long." });
    expect(result.status).toBe("completed");
    expect((result.channels as Record<string, AgentResult>).edited).toBeDefined();
  });

  it("the new fast micro-agents each compile and run to completion", async () => {
    const agents = [
      { make: () => prebuilt.sentimentAnalyzer(), channel: "sentiment" },
      { make: () => prebuilt.entityExtractor(), channel: "entities" },
      { make: () => prebuilt.piiRedactor(), channel: "redacted" },
      { make: () => prebuilt.intentClassifier(), channel: "intent" },
      { make: () => prebuilt.titleGenerator(), channel: "title" },
      { make: () => prebuilt.keywordExtractor(), channel: "keywords" },
      { make: () => prebuilt.questionAnswerer(), channel: "answer" }
    ] as const;

    for (const { make, channel } of agents) {
      const result = await make().run({ question: "Some input text to process." });
      expect(result.status).toBe("completed");
      expect((result.channels as Record<string, AgentResult>)[channel]).toBeDefined();
    }
  });
});
