import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  rustEngineAvailable,
  writeTodosTool
} from "./index.js";

/**
 * End-to-end wire proof for the `writeTodos` deep-agent tool (ADR 0022/0023, phase 1)
 * on the **Rust engine**. With no provider keys the bridge drives the agent against a
 * deterministic mock that returns a plain answer (it does not script a `writeTodos`
 * call — that scriptable path is covered in the Rust crate's `node.rs` test, which
 * proves the durable `__todos` persistence). What this asserts is that the rebuilt
 * native addon ACCEPTS the new surface across the napi/serde boundary: an agent node
 * carrying the `writeTodos` tool + a `todosChannel` config compiles and runs to
 * completion (no deserialization / registration error). Skips when the addon is absent.
 */
const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu/graph-sdk — writeTodos tool on the Rust engine", () => {
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

  it("accepts an agent with the writeTodos tool + a durable todosChannel and runs to completion", async () => {
    const tools = new InMemoryToolRegistry();
    tools.register(writeTodosTool.definition, writeTodosTool.handler);

    const app = createGraph({ name: "rust-write-todos" })
      .channel("__todos", { type: "json", reducer: "replace" })
      .agentNode("planner", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Plan with writeTodos, then act." },
        tools,
        todosChannel: "__todos",
        maxIterations: 1
      })
      .compile();

    expect(app.usesRustEngine).toBe(true);

    const result = await app.run({ question: "ship the feature" }, { runId: "run_write_todos" as never });
    expect(result.status).toBe("completed");
  });
});
