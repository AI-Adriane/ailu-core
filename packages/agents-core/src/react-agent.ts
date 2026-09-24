import type { Agent } from "./interfaces.js";
import type { AgentId, AgentResult } from "./types.js";
import type { ToolRegistry } from "./tools.js";
import type { LLMGateway } from "../../llm-gateway/src/interfaces.js";
import type { LLMContentBlock, LLMMessage, LLMProvider, LLMToolDef } from "../../llm-gateway/src/types.js";
import type { PromptRegistry } from "../../llm-gateway/src/prompt-registry.js";
import type { BaseStore } from "../../memory-store/src/interfaces.js";
import type { CallbackManager } from "../../callbacks/src/interfaces.js";
import type { GraphState } from "@ailu-ai/graph-core";
import type { WorkingMemory } from "./working-memory.js";

const DEFAULT_PROVIDER: LLMProvider = "anthropic";
const DEFAULT_MODEL = "claude-opus-4-8";

type ReActAgentOptions = {
  id: AgentId;
  name: string;
  description: string;
  llm: LLMGateway;
  tools?: ToolRegistry;
  maxIterations?: number;
  provider?: LLMProvider;
  model?: string;
  /** System prompt source — agents reference prompts by id, never inline them. */
  promptRegistry?: PromptRegistry;
  promptId?: string;
  promptVersion?: string;
  /**
   * Names of `requiresApproval` tools that have already been granted human
   * approval (e.g. injected into state on resume). Listed tools execute instead
   * of being gated again — this is how a suspended-for-approval run continues.
   */
  approvedToolNames?: string[];
};

export class ReActAgent<TInput> implements Agent<TInput> {
  public readonly id: AgentId;
  public readonly name: string;
  public readonly description: string;
  private readonly llm: LLMGateway;
  private readonly tools?: ToolRegistry;
  private readonly maxIterations: number;
  private readonly provider: LLMProvider;
  private readonly model: string;
  private readonly promptRegistry?: PromptRegistry;
  private readonly promptId?: string;
  private readonly promptVersion?: string;
  private readonly approvedToolNames: Set<string>;

  public constructor(options: ReActAgentOptions) {
    this.id = options.id;
    this.name = options.name;
    this.description = options.description;
    this.llm = options.llm;
    this.tools = options.tools;
    this.maxIterations = options.maxIterations ?? 6;
    this.provider = options.provider ?? DEFAULT_PROVIDER;
    this.model = options.model ?? DEFAULT_MODEL;
    this.promptRegistry = options.promptRegistry;
    this.promptId = options.promptId;
    this.promptVersion = options.promptVersion;
    this.approvedToolNames = new Set(options.approvedToolNames ?? []);
  }

  public async run(
    input: TInput,
    state: GraphState,
    context: { memory: BaseStore; workingMemory: WorkingMemory; callbacks?: CallbackManager }
  ): Promise<AgentResult> {
    const trace: string[] = [];
    const approvalRequests: AgentResult["approvalRequests"] = [];
    // Static, cacheable prefix resolved from the registry; dynamic state is sent
    // in the per-turn user message so it never busts the cached prefix.
    const system = this.resolveSystemPrompt();
    const toolDefs = this.buildToolDefs();

    // A real multi-turn conversation: the static system/tools form the cacheable
    // prefix; tool results come back as structured `tool_result` blocks paired to
    // their `tool_use` by id, rather than being flattened into a text trace.
    const conversation: LLMMessage[] = [
      { role: "user", content: `Input: ${JSON.stringify(input)}\nState: ${JSON.stringify(state.channels)}` }
    ];

    for (let i = 0; i < this.maxIterations; i += 1) {
      const completion = await this.llm.complete({
        provider: this.provider,
        model: this.model,
        system,
        tools: toolDefs,
        messages: conversation
      });
      const content = completion.content.trim();
      trace.push(`thought:${content}`);

      // Native tool-calling: when the provider surfaces structured tool_use blocks,
      // record the assistant turn, run the tools, and feed results back as
      // tool_result blocks. An approval-gated tool stops the loop (no self-approval).
      const toolCalls = completion.toolCalls ?? [];
      if (toolCalls.length > 0) {
        const assistantBlocks: LLMContentBlock[] = [];
        if (content.length > 0) {
          assistantBlocks.push({ type: "text", text: content });
        }
        for (const call of toolCalls) {
          assistantBlocks.push({ type: "tool_use", id: call.id, name: call.name, input: call.input });
        }
        conversation.push({ role: "assistant", content: assistantBlocks });

        const resultBlocks: LLMContentBlock[] = [];
        let approvalHit = false;
        for (const call of toolCalls) {
          const result = await this.executeToolCall(
            call.name,
            call.input,
            { trace, approvalRequests, callbacks: context.callbacks },
            state
          );
          if (result.status === "approval") {
            approvalHit = true;
            break;
          }
          resultBlocks.push({
            type: "tool_result",
            toolUseId: call.id,
            content: result.output,
            isError: result.status === "not_found"
          });
        }
        if (approvalHit) {
          break;
        }
        conversation.push({ role: "user", content: resultBlocks });
        continue;
      }

      // Record the assistant's text turn before deciding how to proceed.
      if (content.length > 0) {
        conversation.push({ role: "assistant", content });
      }

      // ACTION: text-protocol tool call — execute, feed the observation back, loop.
      if (content.startsWith("ACTION:")) {
        const [, toolNameRaw, payloadRaw] = content.match(/^ACTION:\s+(\S+)\s*(.*)$/) ?? [];
        const toolName = toolNameRaw ?? "";
        const payloadText = payloadRaw ?? "";
        const payload = payloadText.length > 0 ? JSON.parse(payloadText) : {};
        const result = await this.executeToolCall(
          toolName,
          payload,
          { trace, approvalRequests, callbacks: context.callbacks },
          state
        );
        if (result.status === "approval") {
          break;
        }
        // Feed the observation back so the next turn can use it (text protocol).
        conversation.push({ role: "user", content: `observation:${result.output}` });
        continue;
      }

      // Any tool-call-free, action-free text turn is the final answer. Honour an
      // explicit `FINAL:` marker wherever it appears (models rarely put it first),
      // otherwise take the whole text. We must NOT loop here: re-querying would
      // append an assistant-terminated conversation, which strict providers
      // (e.g. Mistral / OpenAI-compatible) reject with a 400.
      const finalIndex = content.indexOf("FINAL:");
      const answer = finalIndex >= 0 ? content.slice(finalIndex + "FINAL:".length).trim() : content;
      trace.push(`final:${answer}`);
      break;
    }

    await context.memory.put(["agent", this.name], "last-react-trace", trace);
    await context.memory.put(
      ["agent", this.name],
      "short-term-count",
      context.workingMemory.shortTerm.length
    );

    return {
      artifacts: [],
      blockers: [],
      approvalRequests,
      confidence: 0.7,
      reasoning: trace.join("\n"),
      requiresHumanReview: approvalRequests.length > 0
    };
  }

  /**
   * Resolve a tool by name and either execute it or gate it. Shared by the native
   * tool_use path and the legacy ACTION: text protocol so both honour the approval
   * rule identically: a `requiresApproval` tool is never self-executed.
   */
  private async executeToolCall(
    name: string,
    input: unknown,
    sink: {
      trace: string[];
      approvalRequests: AgentResult["approvalRequests"];
      callbacks?: CallbackManager;
    },
    state: GraphState
  ): Promise<{ status: "approval" | "executed" | "not_found"; output: string }> {
    await sink.callbacks?.emit({
      type: "onAgentAction",
      runId: String(state.runId),
      nodeId: String(state.currentNodeId),
      timestamp: new Date().toISOString(),
      action: name,
      payload: input
    });

    const resolved = this.tools?.resolve(name as never);
    if (resolved === undefined) {
      sink.trace.push(`observation:tool_not_found:${name}`);
      return { status: "not_found", output: `tool_not_found:${name}` };
    }

    // Gate sensitive tools — unless this exact tool was already approved by a human
    // (e.g. granted on resume), in which case it runs.
    if (resolved.definition.requiresApproval === true && !this.approvedToolNames.has(resolved.definition.name)) {
      sink.approvalRequests.push({
        subject: { description: `tool:${resolved.definition.name}` },
        reason: `Tool '${resolved.definition.name}' requires human approval before execution.`
      });
      sink.trace.push(`observation:approval_required:${name}`);
      return { status: "approval", output: "" };
    }

    const parsedInput = resolved.definition.inputSchema.parse(input);
    const toolOutput = await resolved.handler(parsedInput);
    const parsedOutput = resolved.definition.outputSchema.parse(toolOutput);
    const output = JSON.stringify(parsedOutput);
    sink.trace.push(`observation:${output}`);
    return { status: "executed", output };
  }

  private resolveSystemPrompt(): string | undefined {
    if (this.promptRegistry === undefined || this.promptId === undefined) {
      return undefined;
    }
    return this.promptRegistry.get(this.promptId, this.promptVersion).system;
  }

  private buildToolDefs(): LLMToolDef[] | undefined {
    if (this.tools === undefined) {
      return undefined;
    }
    const defs = this.tools
      .list()
      .filter((tool) => tool.jsonSchema !== undefined)
      .map((tool) => ({
        name: tool.name,
        description: tool.description,
        inputSchema: tool.jsonSchema as Record<string, unknown>
      }));
    return defs.length > 0 ? defs : undefined;
  }
}
