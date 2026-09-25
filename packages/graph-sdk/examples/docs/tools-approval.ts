import assert from "node:assert/strict";

// #region example
import { createGraph, InMemoryToolRegistry, model, type ToolId } from "@ailu-ai/graph-sdk";

const refunds: string[] = [];

// 1. Describe the tool. `jsonSchema` and `description` are what the model sees.
const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Refund an order. Use only when the customer asks for a refund.",
    jsonSchema: {
      type: "object",
      properties: { orderId: { type: "string" } },
      required: ["orderId"]
    },
    inputSchema: { parse: (value: unknown) => value as { orderId?: string } },
    outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
    permissions: ["payments:write"],
    requiresApproval: true // a human must approve every call
  },
  async (input) => {
    refunds.push(input.orderId ?? "unknown");
    return { ok: true };
  }
);

// 2. Give the tool to an agent that stops when it wants to call a gated tool.
const app = createGraph({ name: "support" })
  .channel("request", { type: "string", default: "" })
  .agentNode("assistant", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "Help the customer. Use the refund tool when needed." },
    tools,
    suspendForApproval: true
  })
  .compile();

// 3. The run suspends before the refund runs.
const paused = await app.run({ request: "Please refund order ORD-8830." });
console.log(paused.status); // "suspended"
console.log(refunds.length); // 0: nothing ran yet

// 4. A named human approves; the agent resumes and the refund runs.
const done = await app.approveAndResume(paused.runId, {
  approvedTools: ["refund"],
  resolvedBy: "alice@example.com"
});
console.log(done.status); // "completed"
console.log(refunds.length); // 1
// #endregion example

assert.equal(paused.status, "suspended");
assert.equal(done.status, "completed");
assert.equal(refunds.length, 1);
