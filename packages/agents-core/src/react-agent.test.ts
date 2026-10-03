import { describe, expect, it, vi } from "vitest";
import { DefaultLLMGateway, InMemoryPromptRegistry } from "../../llm-gateway/src/index.js";
import type {
  LLMProvider,
  LLMProviderAdapter,
  LLMRequest,
  LLMResponse,
  LLMStreamChunk
} from "../../llm-gateway/src/index.js";
import { InMemoryStore } from "../../memory-store/src/in-memory-store.js";

import { ReActAgent } from "./react-agent.js";
import { InMemoryToolRegistry, type ToolDefinition, type ToolId } from "./tools.js";
import type { WorkingMemory } from "./working-memory.js";

/** Adapter that records every request and replays a scripted list of contents. */
class RecordingAdapter implements LLMProviderAdapter {
  public readonly provider: LLMProvider = "anthropic";
  public readonly requests: LLMRequest[] = [];
  private index = 0;

  public constructor(private readonly contents: string[]) {}

  public async complete(req: LLMRequest): Promise<LLMResponse> {
    this.requests.push(req);
    const content = this.contents[Math.min(this.index, this.contents.length - 1)] ?? "FINAL: done";
    this.index += 1;
    return {
      content,
      usage: { promptTokens: 1, completionTokens: 1 },
      model: req.model,
      provider: this.provider
    };
  }

  public async *stream(): AsyncIterable<LLMStreamChunk> {
    yield { delta: "", done: true };
  }
}

/** Adapter that replays scripted full responses — used to drive native tool_use turns. */
class ScriptedAdapter implements LLMProviderAdapter {
  public readonly provider: LLMProvider = "anthropic";
  public readonly requests: LLMRequest[] = [];
  private index = 0;

  public constructor(private readonly responses: LLMResponse[]) {}

  public async complete(req: LLMRequest): Promise<LLMResponse> {
    this.requests.push(req);
    const response = this.responses[Math.min(this.index, this.responses.length - 1)];
    this.index += 1;
    return (
      response ?? {
        content: "FINAL: done",
        usage: { promptTokens: 1, completionTokens: 1 },
        model: req.model,
        provider: this.provider
      }
    );
  }

  public async *stream(): AsyncIterable<LLMStreamChunk> {
    yield { delta: "", done: true };
  }
}

const passthrough = { parse: (input: unknown) => input };

const gatewayWith = (adapter: RecordingAdapter): DefaultLLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(adapter);
  return gateway;
};

const runContext = () => {
  const memory = new InMemoryStore();
  const workingMemory: WorkingMemory = { shortTerm: [], longTerm: memory };
  return { memory, workingMemory };
};

describe("ReActAgent", () => {
  it("stops the loop when a final answer appears", async () => {
    const adapter = new RecordingAdapter(["FINAL: done"]);
    const agent = new ReActAgent<string>({
      id: "react-1" as ToolId as never,
      name: "react",
      description: "react",
      llm: gatewayWith(adapter),
      tools: new InMemoryToolRegistry(),
      maxIterations: 3
    });

    const result = await agent.run("goal", {} as never, runContext());

    expect(result.reasoning).toContain("FINAL");
    expect(result.requiresHumanReview).toBe(false);
    expect(adapter.requests[0]?.provider).toBe("anthropic");
  });

  it("resolves the system prompt from the registry and emits tool defs to the gateway", async () => {
    const prompts = new InMemoryPromptRegistry();
    prompts.register({ id: "react.system", version: "1.0.0", system: "You are a ReAct agent." });

    const tools = new InMemoryToolRegistry();
    const search: ToolDefinition<unknown, unknown> = {
      id: "search" as ToolId,
      name: "search",
      description: "Search the corpus",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object", properties: { query: { type: "string" } } }
    };
    tools.register(search, async () => ({}));

    const adapter = new RecordingAdapter(["FINAL: done"]);
    const agent = new ReActAgent<string>({
      id: "react-2" as ToolId as never,
      name: "react",
      description: "react",
      llm: gatewayWith(adapter),
      tools,
      promptRegistry: prompts,
      promptId: "react.system",
      maxIterations: 2
    });

    await agent.run("goal", {} as never, runContext());

    const req = adapter.requests[0]!;
    expect(req.system).toBe("You are a ReAct agent.");
    expect(req.tools).toEqual([
      {
        name: "search",
        description: "Search the corpus",
        inputSchema: { type: "object", properties: { query: { type: "string" } } }
      }
    ]);
  });

  it("gates an approval-required tool instead of executing it", async () => {
    const handler = vi.fn(async () => ({ ok: true }));
    const tools = new InMemoryToolRegistry();
    const danger: ToolDefinition<unknown, unknown> = {
      id: "delete-prod" as ToolId,
      name: "delete-prod",
      description: "Deletes production data",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: ["admin"],
      requiresApproval: true
    };
    tools.register(danger, handler);

    const adapter = new RecordingAdapter(["ACTION: delete-prod {}"]);
    const agent = new ReActAgent<string>({
      id: "react-3" as ToolId as never,
      name: "react",
      description: "react",
      llm: gatewayWith(adapter),
      tools,
      maxIterations: 3
    });

    const result = await agent.run("goal", {} as never, runContext());

    expect(handler).not.toHaveBeenCalled();
    expect(result.requiresHumanReview).toBe(true);
    expect(result.approvalRequests).toEqual([
      {
        subject: { description: "tool:delete-prod" },
        reason: "Tool 'delete-prod' requires human approval before execution."
      }
    ]);
  });

  it("never lets a name grant unlock a tool with conditions — the Rust engine decides those per call", async () => {
    // ADR 0046: a conditioned tool's grant is one call, which this TypeScript agent cannot pin;
    // fail-closed, it stays gated even when its name was granted.
    const handler = vi.fn(async () => ({ ok: true }));
    const tools = new InMemoryToolRegistry();
    const refund: ToolDefinition<unknown, unknown> = {
      id: "refund" as ToolId,
      name: "refund",
      description: "Refunds an order",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: ["payments"],
      requiresApproval: true,
      approvalWhen: [{ argument: "amount", above: 500 }]
    };
    tools.register(refund, handler);

    const agent = new ReActAgent<string>({
      id: "react-conditioned" as ToolId as never,
      name: "react",
      description: "react",
      llm: gatewayWith(new RecordingAdapter(['ACTION: refund {"amount": 20}'])),
      tools,
      maxIterations: 2,
      approvedToolNames: ["refund"]
    });

    const result = await agent.run("goal", {} as never, runContext());

    expect(handler).not.toHaveBeenCalled();
    expect(result.requiresHumanReview).toBe(true);
  });

  it("executes a tool from native tool_use blocks, then finalizes", async () => {
    const handler = vi.fn(async () => ({ temperature: 21 }));
    const tools = new InMemoryToolRegistry();
    const weather: ToolDefinition<unknown, unknown> = {
      id: "weather" as ToolId,
      name: "weather",
      description: "Current weather",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: [],
      jsonSchema: { type: "object" }
    };
    tools.register(weather, handler);

    const adapter = new ScriptedAdapter([
      {
        content: "",
        toolCalls: [{ id: "tu1", name: "weather", input: { city: "Paris" } }],
        stopReason: "tool_use",
        usage: { promptTokens: 1, completionTokens: 1 },
        model: "m",
        provider: "anthropic"
      },
      { content: "FINAL: 21C", usage: { promptTokens: 1, completionTokens: 1 }, model: "m", provider: "anthropic" }
    ]);
    const gateway = new DefaultLLMGateway();
    gateway.registerAdapter(adapter);

    const agent = new ReActAgent<string>({
      id: "react-tool" as ToolId as never,
      name: "react",
      description: "react",
      llm: gateway,
      tools,
      maxIterations: 4
    });

    const result = await agent.run("goal", {} as never, runContext());

    expect(handler).toHaveBeenCalledTimes(1);
    expect(result.reasoning).toContain('observation:{"temperature":21}');
    expect(result.requiresHumanReview).toBe(false);

    // The follow-up turn carries a real multi-turn conversation: the assistant's
    // tool_use turn, then the tool_result fed back to the model (not a text trace).
    const followUp = adapter.requests[1];
    expect(followUp).toBeDefined();
    const assistantTurn = followUp?.messages.find((m) => m.role === "assistant");
    const toolResultTurn = followUp?.messages.find(
      (m) => Array.isArray(m.content) && m.content.some((b) => b.type === "tool_result")
    );
    expect(Array.isArray(assistantTurn?.content)).toBe(true);
    expect(
      Array.isArray(assistantTurn?.content) && assistantTurn.content.some((b) => b.type === "tool_use")
    ).toBe(true);
    expect(toolResultTurn).toBeDefined();
    const resultBlock =
      Array.isArray(toolResultTurn?.content) &&
      toolResultTurn.content.find((b) => b.type === "tool_result");
    expect(resultBlock && resultBlock.type === "tool_result" && resultBlock.content).toContain("21");
  });

  it("gates an approval-required tool surfaced via native tool_use", async () => {
    const handler = vi.fn(async () => ({ ok: true }));
    const tools = new InMemoryToolRegistry();
    const deploy: ToolDefinition<unknown, unknown> = {
      id: "deploy" as ToolId,
      name: "deploy",
      description: "Deploys to production",
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: ["admin"],
      requiresApproval: true
    };
    tools.register(deploy, handler);

    const adapter = new ScriptedAdapter([
      {
        content: "",
        toolCalls: [{ id: "tu1", name: "deploy", input: {} }],
        stopReason: "tool_use",
        usage: { promptTokens: 1, completionTokens: 1 },
        model: "m",
        provider: "anthropic"
      }
    ]);
    const gateway = new DefaultLLMGateway();
    gateway.registerAdapter(adapter);

    const agent = new ReActAgent<string>({
      id: "react-gate" as ToolId as never,
      name: "react",
      description: "react",
      llm: gateway,
      tools,
      maxIterations: 3
    });

    const result = await agent.run("goal", {} as never, runContext());

    expect(handler).not.toHaveBeenCalled();
    expect(result.requiresHumanReview).toBe(true);
    expect(result.approvalRequests[0]?.reason).toContain("deploy");
  });
});
