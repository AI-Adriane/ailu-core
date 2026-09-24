import type { ToolId, ToolRegistry } from "../../agents-core/src/tools.js";
import type { Message } from "@ailu-ai/graph-core";

import { DynamicInterrupt } from "./interrupt.js";
import type { NodeHandler } from "./interfaces.js";

type CreateToolNodeOptions = {
  parallel?: boolean;
};

type ToolCallPayload = {
  id: string;
  name: string;
  input: unknown;
};

type ToolExceptionPayload = {
  toolId: ToolId;
  originalError: unknown;
};

export class ToolException extends Error {
  public readonly toolId: ToolId;
  public readonly originalError: unknown;

  public constructor(payload: ToolExceptionPayload) {
    const message =
      payload.originalError instanceof Error ? payload.originalError.message : "Unknown tool error.";
    super(message);
    this.name = "ToolException";
    this.toolId = payload.toolId;
    this.originalError = payload.originalError;
  }
}

const now = (): Date => new Date();

const createToolMessage = (toolCallId: string, content: string): Message => ({
  id: `tool-msg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}` as Message["id"],
  role: "tool",
  toolCallId,
  content,
  createdAt: now()
});

export const createToolNode =
  (registry: ToolRegistry, options: CreateToolNodeOptions = {}): NodeHandler =>
  async (input) => {
    const channels = input as Record<string, unknown>;
    const messages = Array.isArray(channels.messages) ? (channels.messages as Message[]) : [];
    const lastAiMessage = [...messages].reverse().find((message) => message.role === "ai");
    const toolCalls = Array.isArray((lastAiMessage as { toolCalls?: unknown } | undefined)?.toolCalls)
      ? (((lastAiMessage as { toolCalls?: unknown }).toolCalls as ToolCallPayload[]) ?? [])
      : [];

    const executeOne = async (toolCall: ToolCallPayload): Promise<Message> => {
      const resolved = registry.resolve(toolCall.name as ToolId);
      if (resolved === undefined) {
        throw new ToolException({
          toolId: toolCall.name as ToolId,
          originalError: new Error(`Tool '${toolCall.name}' not found.`)
        });
      }
      const { definition, handler } = resolved;

      if (definition.requiresApproval) {
        throw new DynamicInterrupt("tool-approval-required", {
          approvalRequests: [
            {
              toolId: definition.id,
              toolCallId: toolCall.id,
              reason: `Approval required for tool '${definition.name}'.`
            }
          ]
        });
      }

      const parsedInput = definition.inputSchema.parse(toolCall.input);
      const rawOutput = await handler(parsedInput);
      const parsedOutput = definition.outputSchema.parse(rawOutput);

      return createToolMessage(toolCall.id, JSON.stringify(parsedOutput));
    };

    try {
      const toolMessages = options.parallel
        ? await Promise.all(toolCalls.map((toolCall) => executeOne(toolCall)))
        : await toolCalls.reduce<Promise<Message[]>>(async (previous, toolCall) => {
            const acc = await previous;
            const next = await executeOne(toolCall);
            return [...acc, next];
          }, Promise.resolve([]));

      return { messages: toolMessages };
    } catch (error) {
      const toolError = error instanceof ToolException ? error : undefined;
      if (error instanceof DynamicInterrupt) {
        throw error;
      }
      const toolMessage = createToolMessage(
        toolError?.toolId ?? "tool-error",
        `Tool execution error: ${error instanceof Error ? error.message : "Unknown error"}`
      );
      return { messages: [toolMessage] };
    }
  };
