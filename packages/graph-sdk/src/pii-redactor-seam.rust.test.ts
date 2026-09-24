import { createServer, type Server } from "node:http";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { createGraph, DefaultLLMGateway, rustEngineAvailable } from "./index.js";

/**
 * End-to-end proof of the native PII gateway seam (ADR 0008 phase 2). When
 * `AILU_PII_REDACTOR_URL` is set, the Rust engine wraps its gateway so every
 * intermediate LLM request is POSTed to the redaction service before a provider sees it.
 *
 * We stand up a tiny server implementing the wire contract (`{ texts } -> { texts }`),
 * point the engine at it, run an agent node on the **Rust path**, and assert the server
 * was called with the agent's own system prompt — i.e. the seam is live in the rebuilt
 * native addon. Skips when the addon is absent.
 */
const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu/graph-sdk — native PII redactor seam", () => {
  const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"] as const;
  const saved: Record<string, string | undefined> = {};
  let server: Server;
  let received: string[][] = [];
  let shouldBlock = false;

  beforeEach(async () => {
    received = [];
    shouldBlock = false;
    server = createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => {
        body += chunk;
      });
      req.on("end", () => {
        const parsed = JSON.parse(body) as { texts: string[] };
        received.push(parsed.texts);
        // Redact emails so the round-trip is observably transformed.
        const texts = parsed.texts.map((text) => text.replace(/[\w.]+@[\w.]+/g, "[EMAIL]"));
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ texts, blocked: shouldBlock }));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    const port = typeof address === "object" && address !== null ? address.port : 0;

    saved.AILU_SDK_ENGINE = process.env.AILU_SDK_ENGINE;
    saved.AILU_PII_REDACTOR_URL = process.env.AILU_PII_REDACTOR_URL;
    process.env.AILU_SDK_ENGINE = "rust";
    process.env.AILU_PII_REDACTOR_URL = `http://127.0.0.1:${port}/pii/redact-batch`;
    for (const key of PROVIDER_KEYS) {
      saved[key] = process.env[key];
      delete process.env[key];
    }
  });

  afterEach(async () => {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
    await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  });

  it("POSTs the agent's outbound texts to the redaction service before the provider", async () => {
    const app = createGraph({ name: "rust-agent-pii-seam" })
      .agentNode("assistant", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Reach me at alice@example.com when done." },
        maxIterations: 1
      })
      .compile();

    expect(app.usesRustEngine).toBe(true);

    const result = await app.run({ question: "hi" }, { runId: "run_pii_seam" as never });

    expect(result.status).toBe("completed");
    // The seam fired: the service received at least one batch...
    expect(received.length).toBeGreaterThan(0);
    // ...and the agent's system prompt (with its email) was in the outbound payload.
    const allTexts = received.flat();
    expect(allTexts.some((text) => text.includes("alice@example.com"))).toBe(true);
  });

  it("fails the run when the redaction service blocks (block-level policy)", async () => {
    shouldBlock = true;
    const app = createGraph({ name: "rust-agent-pii-block" })
      .agentNode("assistant", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Reach me at alice@example.com when done." },
        maxIterations: 1
      })
      .compile();

    // The agent node catches a failed agent run and writes the error to its output channel
    // (graceful degradation — a blocked agent does not crash the whole graph). So the block
    // surfaces as an error result instead of a normal answer: the PII never reached a provider.
    const result = await app.run({ question: "hi" }, { runId: "run_pii_block" as never });
    const agentResult = (result.channels as Record<string, { error?: string }>).agentResult;

    expect(received.length).toBeGreaterThan(0);
    expect(typeof agentResult?.error).toBe("string");
    expect(agentResult?.error ?? "").toMatch(/block/i);
  });
});
