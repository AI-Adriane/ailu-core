/**
 * Agent + approval — the core governance loop.
 *
 * A support agent reaches for a sensitive tool (`refund`). The tool is registered with
 * `requiresApproval: true`, so the agent cannot run it on its own authority: with
 * `suspendForApproval` the whole run suspends at the agent node and nothing is refunded.
 * A human then grants the tool with `approveAndResume`; the run resumes and the refund
 * executes exactly once. The approver is recorded, and the engine refuses an approval
 * granted by the agent that asked for it.
 *
 * Run it offline (no API key — the engine's deterministic mock calls each declared tool once):
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:agent
 * With ANTHROPIC_API_KEY set, the same code runs on Claude.
 */
import { createGraph, InMemoryToolRegistry, model, type ToolId } from "@ailu-ai/graph-sdk";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

// ── The sensitive tool ───────────────────────────────────────────────────────
let refunds = 0;
const passthrough = { parse: (value: unknown) => value };

const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Refund a customer order. Sensitive: needs human approval.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["payments:write"],
    requiresApproval: true, // the agent can request it, never run it unapproved
    jsonSchema: { type: "object", properties: { orderId: { type: "string" } } } // what the model sees
  },
  async (input: unknown) => {
    refunds += 1;
    console.log("  → refund executed", JSON.stringify(input));
    return { ok: true };
  }
);

// ── The graph: one agent node ────────────────────────────────────────────────
const app = createGraph({ name: "support-agent" })
  .channel("ticket", { type: "string", default: "" })
  .agentNode("assistant", {
    model: model.anthropic("claude-sonnet-4-6"),
    prompt: { system: "You are a support agent. Use your tools to resolve the ticket." },
    tools,
    suspendForApproval: true, // suspend the run when a tool needs approval
    maxIterations: 4
  })
  .compile();

// 1) The agent reaches for `refund` → the run suspends; nothing has been refunded.
const suspended = await app.run({ ticket: "Order #1042 arrived broken. Please refund it." });
console.log("status:", suspended.status); // "suspended"
check(suspended.status === "suspended", "the run suspended for approval");
check(String(suspended.currentNodeId) === "assistant", "it is paused at the agent node");
check(
  JSON.stringify(suspended.channels.agentResult.approvalRequests).includes("tool:refund"),
  "the pending approval request names tool:refund"
);
check(refunds === 0, "the refund did NOT run before approval");

// 2) A human (alice) grants the tool; the run resumes and the refund executes.
const done = await app.approveAndResume(suspended.runId, {
  approvedTools: ["refund"],
  resolvedBy: "alice" // the approver's identity; the requesting agent can never approve itself
});
console.log("resumed status:", done.status); // "completed"
check(done.status === "completed", "the run completed after approval");
check(refunds === 1, "the refund ran exactly once");
