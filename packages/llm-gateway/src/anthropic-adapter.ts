// ADR 0034 (16a): `@anthropic-ai/sdk` is a TYPE-only import here (erased at build) plus a single
// lazy runtime `require` inside `createDefaultPort` — so importing this module (e.g. the mock
// `DefaultLLMGateway` path) never pulls the SDK, and `@ailu-ai/graph-sdk` ships without it.
// This is the deprecated TS fallback; the Rust engine is the real provider path.
import type Anthropic from "@anthropic-ai/sdk";
import { createRequire } from "node:module";

import type { LLMProviderAdapter } from "./interfaces.js";
import type { LLMContentBlock, LLMRequest, LLMResponse, LLMStreamChunk } from "./types.js";

const DEFAULT_MODEL = "claude-opus-4-8";
const DEFAULT_MAX_TOKENS = 16000;
const DEFAULT_STREAM_MAX_TOKENS = 64000;

/**
 * Provider-shaped request the adapter assembles. This is the cache seam: the
 * `system` and `tools` blocks carry the cache_control breakpoints and must stay
 * byte-stable across calls. The default port translates this into the SDK request;
 * tests fake the port and assert on this shape directly.
 */
export type AnthropicCreateParams = {
  model: string;
  maxTokens: number;
  system?: Array<{ type: "text"; text: string; cacheable?: boolean }>;
  tools?: Array<{
    name: string;
    description?: string;
    inputSchema: Record<string, unknown>;
    cacheable?: boolean;
  }>;
  messages: Array<{ role: "user" | "assistant"; content: string | LLMContentBlock[] }>;
};

/** Structural subset of the SDK `Message` the adapter actually reads. */
export type AnthropicRawResponse = {
  content: Array<{ type: string; text?: string; id?: string; name?: string; input?: unknown }>;
  stop_reason?: string | null;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_read_input_tokens?: number | null;
    cache_creation_input_tokens?: number | null;
  };
};

/**
 * The only seam onto the Anthropic SDK. The default implementation wraps a real
 * client; tests supply a fake so the cache + accounting logic is covered without
 * a network call or an API key.
 */
export interface AnthropicClientPort {
  create(params: AnthropicCreateParams): Promise<AnthropicRawResponse>;
  stream(params: AnthropicCreateParams): AsyncIterable<LLMStreamChunk>;
}

export type AnthropicAdapterOptions = {
  /** Override the model used when the request does not name a Claude model. */
  defaultModel?: string;
  /** Inject a client port (tests) or an API key (production). */
  port?: AnthropicClientPort;
  apiKey?: string;
};

export class AnthropicProviderAdapter implements LLMProviderAdapter {
  public readonly provider = "anthropic" as const;
  private readonly port: AnthropicClientPort;
  private readonly defaultModel: string;

  public constructor(options: AnthropicAdapterOptions = {}) {
    this.defaultModel = options.defaultModel ?? DEFAULT_MODEL;
    this.port = options.port ?? createDefaultPort(options.apiKey);
  }

  public async complete(req: LLMRequest): Promise<LLMResponse> {
    const params = this.buildParams(req, req.maxTokens ?? DEFAULT_MAX_TOKENS);
    const raw = await this.port.create(params);
    return this.toResponse(req, params.model, raw);
  }

  public async *stream(req: LLMRequest): AsyncIterable<LLMStreamChunk> {
    const params = this.buildParams(req, req.maxTokens ?? DEFAULT_STREAM_MAX_TOKENS);
    yield* this.port.stream(params);
  }

  /**
   * Assemble the provider request. The cacheable prefix is `tools` then `system`
   * (Anthropic render order); a breakpoint on the last tool and on the system block
   * caches that prefix. Sampling params are intentionally dropped — Opus 4.7/4.8
   * reject `temperature`/`top_p`/`top_k`. No date/timestamp is added here.
   */
  private buildParams(req: LLMRequest, maxTokens: number): AnthropicCreateParams {
    const systemText = this.collectSystem(req);
    const params: AnthropicCreateParams = {
      model: this.resolveModel(req.model),
      maxTokens,
      messages: req.messages
        .filter((m) => m.role !== "system")
        .map((m) => ({ role: (m.role === "assistant" ? "assistant" : "user") as "user" | "assistant", content: m.content }))
    };

    if (systemText.length > 0) {
      params.system = [{ type: "text", text: systemText, cacheable: true }];
    }

    if (req.tools !== undefined && req.tools.length > 0) {
      params.tools = req.tools.map((tool, index) => ({
        name: tool.name,
        description: tool.description,
        inputSchema: tool.inputSchema,
        // Breakpoint on the last tool caches the whole deterministic tool list.
        cacheable: index === req.tools!.length - 1
      }));
    }

    return params;
  }

  private collectSystem(req: LLMRequest): string {
    const parts: string[] = [];
    if (req.system !== undefined && req.system.length > 0) {
      parts.push(req.system);
    }
    for (const message of req.messages) {
      if (message.role === "system" && typeof message.content === "string") {
        parts.push(message.content);
      }
    }
    return parts.join("\n\n");
  }

  private resolveModel(model: string): string {
    return model.startsWith("claude-") ? model : this.defaultModel;
  }

  private toResponse(req: LLMRequest, model: string, raw: AnthropicRawResponse): LLMResponse {
    const content = raw.content
      .filter((block) => block.type === "text")
      .map((block) => block.text ?? "")
      .join("");

    const toolCalls = raw.content
      .filter((block) => block.type === "tool_use")
      .map((block) => ({
        id: block.id ?? "",
        name: block.name ?? "",
        input: block.input ?? {}
      }));

    return {
      content,
      ...(toolCalls.length > 0 ? { toolCalls } : {}),
      ...(raw.stop_reason != null ? { stopReason: raw.stop_reason } : {}),
      usage: {
        promptTokens: raw.usage.input_tokens,
        completionTokens: raw.usage.output_tokens,
        cacheReadTokens: raw.usage.cache_read_input_tokens ?? 0,
        cacheWriteTokens: raw.usage.cache_creation_input_tokens ?? 0
      },
      model,
      provider: req.provider
    };
  }
}

const ephemeral: Anthropic.CacheControlEphemeral = { type: "ephemeral" };

/** Constructor shape of the SDK default export — typed from the (erased) type import. */
type AnthropicCtor = new (opts?: { apiKey?: string }) => Anthropic;

let cachedAnthropicCtor: AnthropicCtor | undefined;

/** Lazily load the `@anthropic-ai/sdk` constructor (ADR 0034 16a). A static `createRequire` of a
 * constant package name — not a dynamic import of a user string. Throws a clear error if the
 * optional SDK is not installed (the engine is the default path; this is the TS fallback). */
const anthropicCtor = (): AnthropicCtor => {
  if (cachedAnthropicCtor === undefined) {
    try {
      const requireFn = createRequire(import.meta.url);
      const mod = requireFn("@anthropic-ai/sdk") as { default?: AnthropicCtor } & AnthropicCtor;
      cachedAnthropicCtor = (mod.default ?? mod) as AnthropicCtor;
    } catch {
      throw new Error(
        "@anthropic-ai/sdk is not installed. It is an optional dependency: the TS Anthropic " +
          "adapter is the deprecated fallback — install @anthropic-ai/sdk to use it, or run on " +
          "the Rust engine (the default)."
      );
    }
  }
  return cachedAnthropicCtor;
};

/** Wraps a real Anthropic client. This is the only code that touches the SDK (lazily). */
const createDefaultPort = (apiKey?: string): AnthropicClientPort => {
  const Ctor = anthropicCtor();
  const client = new Ctor(apiKey === undefined ? {} : { apiKey });

  const toSdkContent = (
    content: AnthropicCreateParams["messages"][number]["content"]
  ): Anthropic.MessageParam["content"] => {
    if (typeof content === "string") {
      return content;
    }
    return content.map((block) => {
      if (block.type === "text") {
        return { type: "text" as const, text: block.text };
      }
      if (block.type === "tool_use") {
        return { type: "tool_use" as const, id: block.id, name: block.name, input: block.input };
      }
      return {
        type: "tool_result" as const,
        tool_use_id: block.toolUseId,
        content: block.content,
        ...(block.isError === true ? { is_error: true } : {})
      };
    });
  };

  const toSdkParams = (params: AnthropicCreateParams): Anthropic.MessageCreateParamsNonStreaming => {
    const sdk: Anthropic.MessageCreateParamsNonStreaming = {
      model: params.model,
      max_tokens: params.maxTokens,
      messages: params.messages.map((m) => ({ role: m.role, content: toSdkContent(m.content) }))
    };
    if (params.system !== undefined) {
      sdk.system = params.system.map((block) => ({
        type: "text",
        text: block.text,
        ...(block.cacheable === true ? { cache_control: ephemeral } : {})
      }));
    }
    if (params.tools !== undefined) {
      sdk.tools = params.tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        input_schema: { type: "object" as const, ...tool.inputSchema },
        ...(tool.cacheable === true ? { cache_control: ephemeral } : {})
      }));
    }
    return sdk;
  };

  return {
    async create(params) {
      return client.messages.create(toSdkParams(params));
    },
    async *stream(params) {
      const live = client.messages.stream({ ...toSdkParams(params), stream: true });
      for await (const event of live) {
        if (event.type === "content_block_delta" && event.delta.type === "text_delta") {
          yield { delta: event.delta.text, done: false };
        }
      }
      yield { delta: "", done: true };
    }
  };
};
