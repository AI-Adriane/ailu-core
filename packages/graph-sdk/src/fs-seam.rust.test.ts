import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { createGraph, DefaultLLMGateway, rustEngineAvailable } from "./index.js";

/**
 * End-to-end wire proof for the governed virtual filesystem (ADR 0024 phase 2b) on the
 * **Rust engine**. With no provider keys the agent runs against a deterministic mock
 * that returns a plain answer (it does not script fs tool calls — the fs tool behaviour
 * is proven in the Rust `fs-backend` + `agents-core/fs_tools` crate tests). What this
 * asserts is that the rebuilt native addon ACCEPTS the new surface across the napi/serde
 * boundary: a graph with `.fsPolicy([...])` and an `enableFs: true` agent compiles and
 * runs to completion — `fsPolicy` + `enableFs` deserialize and the fs tools register
 * without error. Skips when the addon is absent.
 */
const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu/graph-sdk — governed fs seam on the Rust engine", () => {
  const PROVIDER_KEYS = ["MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "AILU_USE_OLLAMA"] as const;
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
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

  it("accepts a graph with an fs policy + an fs-enabled agent and runs to completion", async () => {
    const app = createGraph({ name: "rust-fs-seam" })
      .fsPolicy([
        { glob: "scratch/**", verb: "write" },
        { glob: "secret/**", verb: "deny" }
      ])
      .agentNode("worker", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Offload context to the filesystem under scratch/." },
        enableFs: true,
        maxIterations: 1
      })
      .compile();

    expect(app.usesRustEngine).toBe(true);

    const result = await app.run({ task: "summarize" }, { runId: "run_fs_seam" as never });
    expect(result.status).toBe("completed");
  });
});
