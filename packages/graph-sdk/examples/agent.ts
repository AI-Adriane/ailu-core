/**
 * Agent + native approval gate.
 *
 * A ReAct agent reaches for a sensitive tool via a structured tool call. Because the
 * tool requires approval the agent refuses to self-approve — the run suspends cleanly
 * at the agent node. A human grants approval and the run resumes, executing the tool.
 * This is the core governance loop Ailu is built around — runnable with a mock LLM,
 * no API key.
 */
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  type LLMGateway,
  type ToolId
} from "@ailu/graph-sdk";

// A mock LLM that always asks to call the `refund` tool (a structured tool call).
const mockLLM = (toolName: string): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new MockLLMProviderAdapter({
      provider: "anthropic",
      response: {
        content: "",
        toolCalls: [{ id: "tu1", name: toolName, input: {} }],
        stopReason: "tool_use",
        usage: { promptTokens: 0, completionTokens: 0 },
        model: "mock",
        provider: "anthropic"
      }
    })
  );
  return gateway;
};

const refundHandler = async (): Promise<{ ok: boolean }> => {
  console.log("→ refund executed");
  return { ok: true };
};

const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Issues a customer refund. Sensitive.",
    inputSchema: { parse: (v: unknown) => v },
    outputSchema: { parse: (v: unknown) => v },
    permissions: ["payments:write"],
    requiresApproval: true,
    jsonSchema: { type: "object" }
  },
  refundHandler
);

const app = createGraph({ name: "support-agent" })
  .agentNode("assistant", {
    llm: mockLLM("refund"),
    prompt: { system: "You are a support agent. Use tools when needed." },
    tools,
    suspendForApproval: true,
    maxIterations: 2
  })
  .compile();

// 1) The agent reaches for `refund` → run suspends for approval (tool not executed).
const suspended = await app.run();
console.log("status:", suspended.status); // "suspended"
console.log("paused at:", suspended.currentNodeId); // "assistant"
console.log("approval requests:", suspended.channels.agentResult?.approvalRequests.length); // 1

// 2) A human grants approval; the run resumes and the tool runs.
const done = await app.approveAndResume(suspended.runId, { approvedTools: ["refund"] });
console.log("resumed status:", done.status); // "completed"
