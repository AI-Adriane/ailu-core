import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  components,
  createGraph,
  InMemoryToolRegistry,
  model,
  type GraphDefinition
} from "./index.js";
import type { ToolId } from "@ailu-ai/agents-core";

/**
 * ADR 0045 M4 — the definitions the TypeScript builder writes, as golden data for the Python
 * builder (`python/tests/test_ailu.py` builds the same graphs with `ailu.create_graph` and must
 * write the same definitions): a graph built in one SDK runs from the other.
 * `AILU_UPDATE_BUILDER_GOLDEN=1` rewrites the file from this builder.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../python/tests/fixtures/builder_golden.json", import.meta.url)
);

const passthrough = { parse: (value: unknown) => value };

const supportGraph = (): GraphDefinition => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "lookup" as ToolId,
      name: "lookup",
      description: "Looks an order up.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { order: { type: "string" } } }
    },
    async () => ({})
  );
  tools.register(
    {
      id: "refund" as ToolId,
      name: "refund",
      description: "Refunds an order.",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      requiresApproval: true
    },
    async () => ({})
  );
  return createGraph({ name: "Support Flow" })
    .channel("ticket", { type: "string", default: "" })
    .channel("prompt", { type: "string" })
    .node("prepare", async () => ({}))
    .component(
      "build",
      components.promptBuilder({ template: "Ticket: {{ticket}}", into: "prompt" })
    )
    .agentNode("assistant", {
      prompt: { system: "Help." },
      tools,
      suspendForApproval: true,
      maxIterations: 3
    })
    .agentNode("fast", { prompt: { system: "Fast." }, tier: "fast", outputChannel: "quick" })
    .humanGate("review")
    .edge("prepare", "build")
    .edge("build", "assistant")
    .conditionalEdge("assistant", "review", "needs_review", () => true)
    .edge("review", "fast")
    .compile().definition;
};

const child = () =>
  createGraph({ name: "Child Task" })
    .channel("in", { type: "string", default: "" })
    .channel("out", { type: "string" })
    .node("child_step", async () => ({}));

const nestedGraph = (): GraphDefinition =>
  createGraph({ name: "Nested", id: "nested-graph", version: "2.0.0" })
    .channel("ticket", { type: "string", default: "" })
    .channel("result", { type: "string" })
    .channel("secret", { type: "string", noLog: true })
    .node("risky", {
      type: "action",
      handler: async () => ({}),
      retryPolicy: { maxAttempts: 3, backoffMs: 10 }
    })
    .node("fallback", async () => ({}))
    .agentNode("terse", {
      prompt: { system: "Short." },
      outputStyle: "terse",
      contextBudget: 4000,
      visibleChannels: ["ticket"],
      outputChannel: "summary"
    })
    .agentNode("auto", {
      prompt: { system: "Any provider." },
      model: model.fast,
      outputChannel: "auto_out"
    })
    .subgraph("sub", child(), { inputMapping: { in: "ticket" }, outputMapping: { result: "out" } })
    .errorEdge("risky", "fallback")
    .edge("risky", "terse")
    .edge("terse", "auto")
    .edge("auto", "sub")
    .entry("risky")
    .compile().definition;

const cases = (): Record<string, unknown> => ({
  support: supportGraph(),
  nested: nestedGraph(),
  child: child().compile().definition
});

describe("@ailu-ai/graph-sdk — builder definitions for the Python builder (ADR 0045 M4)", () => {
  if (process.env.AILU_UPDATE_BUILDER_GOLDEN === "1") {
    it("rewrites the golden file from this builder", () => {
      writeFileSync(FIXTURE, `${JSON.stringify(cases(), null, 2)}\n`);
    });
    return;
  }

  it("writes the recorded definitions", () => {
    expect(JSON.parse(JSON.stringify(cases()))).toEqual(JSON.parse(readFileSync(FIXTURE, "utf8")));
  });
});
