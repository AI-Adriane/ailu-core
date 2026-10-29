import { z } from "zod/v3";

import type { LLMGateway, LLMProviderAdapter } from "./interfaces.js";
import type { LLMRequest, LLMStreamChunk } from "./types.js";
import { LLMProviderNotFoundError, LLMValidationError } from "./errors.js";

const LLMContentBlockSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("text"), text: z.string() }),
  z.object({ type: z.literal("tool_use"), id: z.string().min(1), name: z.string().min(1), input: z.unknown() }),
  z.object({
    type: z.literal("tool_result"),
    toolUseId: z.string().min(1),
    content: z.string(),
    isError: z.boolean().optional()
  })
]);

const LLMMessageSchema = z.object({
  role: z.enum(["system", "user", "assistant"]),
  content: z.union([z.string().min(1), z.array(LLMContentBlockSchema).min(1)])
});

const LLMToolDefSchema = z.object({
  name: z.string().min(1),
  description: z.string().optional(),
  inputSchema: z.record(z.unknown())
});

const LLMRequestSchema = z.object({
  provider: z.enum(["openai", "anthropic", "mistral", "google"]),
  model: z.string().min(1),
  messages: z.array(LLMMessageSchema).min(1),
  system: z.string().optional(),
  tools: z.array(LLMToolDefSchema).optional(),
  maxTokens: z.number().int().positive().optional(),
  temperature: z.number().min(0).max(2).optional(),
  stream: z.boolean().optional()
});

export class DefaultLLMGateway implements LLMGateway {
  private readonly adapters = new Map<string, LLMProviderAdapter>();

  public registerAdapter(adapter: LLMProviderAdapter): void {
    this.adapters.set(adapter.provider, adapter);
  }

  public async complete(req: LLMRequest) {
    this.validateRequest(req);
    const adapter = this.adapters.get(req.provider);
    if (adapter === undefined) {
      throw new LLMProviderNotFoundError(req.provider);
    }

    return adapter.complete(req);
  }

  public async *stream(req: LLMRequest): AsyncIterable<LLMStreamChunk> {
    this.validateRequest(req);
    const adapter = this.adapters.get(req.provider);
    if (adapter === undefined) {
      throw new LLMProviderNotFoundError(req.provider);
    }

    for await (const chunk of adapter.stream(req)) {
      yield chunk;
    }
  }

  private validateRequest(req: LLMRequest): void {
    const result = LLMRequestSchema.safeParse(req);
    if (!result.success) {
      throw new LLMValidationError(result.error.issues.map((issue) => issue.message));
    }
  }
}
