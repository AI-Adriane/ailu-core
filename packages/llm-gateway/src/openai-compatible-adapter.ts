import { outputLimit } from "./output-limit.js";

import type { LLMProviderAdapter } from "./interfaces.js";
import type {
  LLMContentBlock,
  LLMProvider,
  LLMRequest,
  LLMResponse,
  LLMStreamChunk,
  LLMToolCall
} from "./types.js";
import { LLMProviderError } from "./errors.js";

const MISTRAL_BASE_URL = "https://api.mistral.ai/v1";
const MISTRAL_DEFAULT_MODEL = "mistral-small-latest";
const OLLAMA_BASE_URL = "http://localhost:11434/v1";
const OLLAMA_DEFAULT_MODEL = "mistral";
// Gemini exposes an OpenAI-compatible chat/completions surface, so it rides this same adapter,
// registered under the first-class `google` provider (LLM_PROVIDERS). Bearer-keyed (GEMINI_API_KEY).
const GEMINI_BASE_URL = "https://generativelanguage.googleapis.com/v1beta/openai";
const GEMINI_DEFAULT_MODEL = "gemini-2.5-flash";
const OPENAI_BASE_URL = "https://api.openai.com/v1";
const OPENAI_DEFAULT_MODEL = "gpt-4o";

/**
 * The OpenAI `/v1/chat/completions` request body the adapter assembles. This is the
 * seam tests assert against directly: {@link buildRequestBody} is a pure function, so
 * the request mapping is covered without a network call or an API key.
 */
export type OpenAIChatRequestBody = {
  model: string;
  messages: OpenAIChatMessage[];
  tools?: Array<{
    type: "function";
    function: { name: string; description?: string; parameters: Record<string, unknown> };
  }>;
  temperature?: number;
  max_tokens?: number;
  max_completion_tokens?: number;
};

/** A single message in the OpenAI chat shape. */
export type OpenAIChatMessage = {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  tool_call_id?: string;
  tool_calls?: Array<{
    id: string;
    type: "function";
    function: { name: string; arguments: string };
  }>;
};

/** Structural subset of the OpenAI chat completion response the adapter reads. */
export type OpenAIChatResponse = {
  choices: Array<{
    message: {
      content?: string | null;
      tool_calls?: Array<{
        id: string;
        type?: string;
        function: { name: string; arguments: string };
      }> | null;
    };
    finish_reason?: string | null;
  }>;
  usage?: {
    prompt_tokens?: number;
    completion_tokens?: number;
    cache_creation_input_tokens?: number;
    prompt_tokens_details?: { cached_tokens?: number; cache_write_tokens?: number };
  };
};

/**
 * The only seam onto the HTTP transport. The default implementation POSTs the body to
 * `baseUrl + '/chat/completions'` via global `fetch`; tests supply a fake so the
 * request/response mapping is covered without a network call.
 */
export interface OpenAICompatibleTransportPort {
  send(body: OpenAIChatRequestBody): Promise<OpenAIChatResponse>;
}

export type OpenAICompatibleAdapterOptions = {
  /** Provider key this adapter registers under in the gateway map. Default `'mistral'`. */
  provider?: LLMProvider;
  /** API base, e.g. `https://api.mistral.ai/v1` or `http://localhost:11434/v1`. */
  baseUrl: string;
  /** Model used when the request does not name a model id for this provider. */
  defaultModel: string;
  /** Bearer token; omitted for keyless servers such as a local Ollama. */
  apiKey?: string;
  /** Inject a transport port (tests) instead of the default `fetch`-backed one. */
  port?: OpenAICompatibleTransportPort;
};

/**
 * One adapter for any server speaking the OpenAI `/v1/chat/completions` shape. Both
 * a local **Ollama** server (`http://localhost:11434/v1`, keyless) and **Mistral
 * cloud** (`https://api.mistral.ai/v1`, bearer key) are driven by this same class;
 * use {@link OpenAICompatibleProviderAdapter.ollama} / `.mistral` to construct them.
 */
export class OpenAICompatibleProviderAdapter implements LLMProviderAdapter {
  public readonly provider: LLMProvider;
  private readonly port: OpenAICompatibleTransportPort;
  private readonly baseUrl: string;
  private readonly defaultModel: string;

  public constructor(options: OpenAICompatibleAdapterOptions) {
    this.provider = options.provider ?? "mistral";
    this.baseUrl = options.baseUrl;
    this.defaultModel = options.defaultModel;
    this.port = options.port ?? createDefaultPort(options.baseUrl, options.apiKey);
  }

  /** Mistral cloud: bearer-keyed, hosted at `https://api.mistral.ai/v1`. */
  public static mistral(apiKey?: string, model?: string): OpenAICompatibleProviderAdapter {
    return new OpenAICompatibleProviderAdapter({
      provider: "mistral",
      baseUrl: MISTRAL_BASE_URL,
      defaultModel: model ?? MISTRAL_DEFAULT_MODEL,
      ...(apiKey !== undefined ? { apiKey } : {})
    });
  }

  /** Google Gemini via its OpenAI-compatible endpoint; registers under the `google` provider. */
  public static google(apiKey?: string, model?: string): OpenAICompatibleProviderAdapter {
    return new OpenAICompatibleProviderAdapter({
      provider: "google",
      baseUrl: GEMINI_BASE_URL,
      defaultModel: model ?? GEMINI_DEFAULT_MODEL,
      ...(apiKey !== undefined ? { apiKey } : {})
    });
  }

  /** OpenAI cloud (the canonical OpenAI-compatible API); registers under the `openai` provider. */
  public static openai(apiKey?: string, model?: string): OpenAICompatibleProviderAdapter {
    return new OpenAICompatibleProviderAdapter({
      provider: "openai",
      baseUrl: OPENAI_BASE_URL,
      defaultModel: model ?? OPENAI_DEFAULT_MODEL,
      ...(apiKey !== undefined ? { apiKey } : {})
    });
  }

  /**
   * A local Ollama server (keyless, `http://localhost:11434/v1`). Registers under the
   * `'mistral'` provider key so it routes through the same gateway slot as Mistral cloud.
   */
  public static ollama(model?: string, baseUrl?: string): OpenAICompatibleProviderAdapter {
    return new OpenAICompatibleProviderAdapter({
      provider: "mistral",
      baseUrl: baseUrl ?? OLLAMA_BASE_URL,
      defaultModel: model ?? OLLAMA_DEFAULT_MODEL
    });
  }

  public async complete(req: LLMRequest): Promise<LLMResponse> {
    const body = buildRequestBody(req, this.defaultModel);
    const raw = await this.port.send(body);
    return this.toResponse(req, body.model, raw);
  }

  public async *stream(req: LLMRequest): AsyncIterable<LLMStreamChunk> {
    // No incremental SSE seam here: complete once, then surface the text as a single
    // delta followed by a terminal chunk. Keeps the contract without faking streaming.
    const response = await this.complete(req);
    if (response.content.length > 0) {
      yield { delta: response.content, done: false };
    }
    yield { delta: "", done: true };
  }

  private toResponse(req: LLMRequest, model: string, raw: OpenAIChatResponse): LLMResponse {
    const choice = raw.choices[0];
    const content = choice?.message.content ?? "";

    const toolCalls: LLMToolCall[] = (choice?.message.tool_calls ?? []).map((call) => ({
      id: call.id,
      name: call.function.name,
      input: parseArguments(call.function.arguments)
    }));

    const cacheReadTokens = raw.usage?.prompt_tokens_details?.cached_tokens;
    const cacheWriteTokens =
      raw.usage?.prompt_tokens_details?.cache_write_tokens ??
      raw.usage?.cache_creation_input_tokens;

    return {
      content,
      ...(toolCalls.length > 0 ? { toolCalls } : {}),
      ...(choice?.finish_reason != null ? { stopReason: choice.finish_reason } : {}),
      usage: {
        promptTokens: raw.usage?.prompt_tokens ?? 0,
        completionTokens: raw.usage?.completion_tokens ?? 0,
        ...(cacheReadTokens != null ? { cacheReadTokens } : {}),
        ...(cacheWriteTokens != null ? { cacheWriteTokens } : {})
      },
      model,
      provider: req.provider
    };
  }
}

/**
 * Map an {@link LLMRequest} to the OpenAI chat-completions body. Pure, so tests assert
 * on it directly. `req.system` folds in as the first `system` message; block content is
 * flattened pragmatically (text blocks joined; assistant `tool_use` blocks become
 * `tool_calls`; `tool_result` blocks become `role:'tool'` messages keyed by id).
 */
export const buildRequestBody = (req: LLMRequest, defaultModel: string): OpenAIChatRequestBody => {
  const messages: OpenAIChatMessage[] = [];

  if (req.system !== undefined && req.system.length > 0) {
    messages.push({ role: "system", content: req.system });
  }

  for (const message of req.messages) {
    messages.push(...mapMessage(message));
  }

  const body: OpenAIChatRequestBody = {
    model: resolveModel(req.model, defaultModel),
    messages
  };

  if (req.tools !== undefined && req.tools.length > 0) {
    body.tools = req.tools.map((tool) => ({
      type: "function" as const,
      function: {
        name: tool.name,
        ...(tool.description !== undefined ? { description: tool.description } : {}),
        parameters: tool.inputSchema
      }
    }));
  }

  if (req.temperature !== undefined) {
    body.temperature = req.temperature;
  }
  // OpenAI o-series reject max_tokens; other compatible APIs retain their parameter.
  const oSeries = /^o\d+(?:-|$)/.test(body.model.replace(/^openai\//, ""));
  const budgetParameter =
    req.provider === "openai" && oSeries ? "max_completion_tokens" : "max_tokens";
  body[budgetParameter] = outputLimit(body.model, req.maxTokens);

  return body;
};

/**
 * Expand one engine message into one or more OpenAI messages. A `tool_result` block
 * becomes its own `role:'tool'` message (OpenAI keeps tool outputs as standalone
 * messages), so a single engine turn carrying results can fan out to several.
 */
const mapMessage = (message: LLMRequest["messages"][number]): OpenAIChatMessage[] => {
  if (typeof message.content === "string") {
    return [{ role: message.role, content: message.content }];
  }

  const toolResults = message.content.filter(
    (block): block is Extract<LLMContentBlock, { type: "tool_result" }> =>
      block.type === "tool_result"
  );
  const toolUses = message.content.filter(
    (block): block is Extract<LLMContentBlock, { type: "tool_use" }> => block.type === "tool_use"
  );
  const text = message.content
    .filter((block): block is Extract<LLMContentBlock, { type: "text" }> => block.type === "text")
    .map((block) => block.text)
    .join("");

  const messages: OpenAIChatMessage[] = [];

  // tool_result blocks → standalone role:'tool' messages paired by tool_call_id.
  for (const result of toolResults) {
    messages.push({ role: "tool", content: result.content, tool_call_id: result.toolUseId });
  }

  // Assistant tool_use blocks → an assistant message carrying tool_calls.
  if (toolUses.length > 0) {
    messages.push({
      role: "assistant",
      content: text,
      tool_calls: toolUses.map((use) => ({
        id: use.id,
        type: "function" as const,
        function: { name: use.name, arguments: JSON.stringify(use.input ?? {}) }
      }))
    });
  } else if (toolResults.length === 0) {
    // Plain text-only block content: emit a single message preserving the role.
    messages.push({ role: message.role, content: text });
  } else if (text.length > 0) {
    // Mixed text alongside tool_result: keep the text as its own message.
    messages.push({ role: message.role, content: text });
  }

  return messages;
};

/** Keep an explicit model id; otherwise fall back to the provider default. */
const resolveModel = (model: string, defaultModel: string): string => {
  return looksLikeModelId(model) ? model : defaultModel;
};

/**
 * Heuristic for "is this a real model id for this provider" vs an agent placeholder
 * (e.g. `claude-opus-4-8`, `react-agent`). Anthropic ids and the agent's default Claude
 * model don't belong here, so route them onto the provider default instead.
 */
const looksLikeModelId = (model: string): boolean => {
  if (model.length === 0) return false;
  if (model.startsWith("claude-")) return false;
  if (model === "react-agent" || model === "mock" || model === "mock-model") return false;
  return true;
};

/** Tool-call arguments arrive as a JSON string; parse defensively, default to `{}`. */
const parseArguments = (raw: string): unknown => {
  if (raw.length === 0) return {};
  try {
    return JSON.parse(raw);
  } catch {
    return {};
  }
};

/** Wraps global `fetch`. This is the only code that touches the network. */
const createDefaultPort = (baseUrl: string, apiKey?: string): OpenAICompatibleTransportPort => {
  const url = `${baseUrl.replace(/\/$/, "")}/chat/completions`;
  return {
    async send(body) {
      const headers: Record<string, string> = { "content-type": "application/json" };
      if (apiKey !== undefined && apiKey.length > 0) {
        headers.authorization = `Bearer ${apiKey}`;
      }
      const res = await fetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(body)
      });
      if (!res.ok) {
        const text = await res.text().catch(() => "");
        throw new LLMProviderError(res.status, text);
      }
      return (await res.json()) as OpenAIChatResponse;
    }
  };
};
