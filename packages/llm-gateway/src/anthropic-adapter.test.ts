import { describe, expect, it } from "vitest";

import {
  AnthropicProviderAdapter,
  type AnthropicClientPort,
  type AnthropicCreateParams,
  type AnthropicRawResponse
} from "./anthropic-adapter.js";
import type { LLMRequest, LLMStreamChunk } from "./types.js";

const makeResponse = (over?: Partial<AnthropicRawResponse>): AnthropicRawResponse => ({
  content: [{ type: "text", text: "hello" }],
  usage: {
    input_tokens: 100,
    output_tokens: 20,
    cache_read_input_tokens: 0,
    cache_creation_input_tokens: 0
  },
  ...over
});

/** Captures the params the adapter builds and returns a canned response. */
const recordingPort = (response: AnthropicRawResponse) => {
  const calls: AnthropicCreateParams[] = [];
  const port: AnthropicClientPort = {
    async create(params) {
      calls.push(params);
      return response;
    },
    async *stream(params) {
      calls.push(params);
      yield { delta: "hel", done: false } satisfies LLMStreamChunk;
      yield { delta: "lo", done: false } satisfies LLMStreamChunk;
      yield { delta: "", done: true } satisfies LLMStreamChunk;
    }
  };
  return { port, calls };
};

const baseRequest = (over?: Partial<LLMRequest>): LLMRequest => ({
  provider: "anthropic",
  model: "claude-opus-4-8",
  messages: [{ role: "user", content: "Hi" }],
  ...over
});

describe("AnthropicProviderAdapter", () => {
  it("marks the system block and the last tool as cacheable", async () => {
    const { port, calls } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port });

    await adapter.complete(
      baseRequest({
        system: "You are a helpful agent.",
        tools: [
          { name: "search", inputSchema: { query: { type: "string" } } },
          { name: "fetch", inputSchema: { url: { type: "string" } } }
        ]
      })
    );

    const params = calls[0]!;
    expect(params.system?.[0]?.cacheable).toBe(true);
    // Only the last tool carries the breakpoint — it caches the whole list.
    expect(params.tools?.map((t) => t.cacheable)).toEqual([false, true]);
  });

  it("maps usage including cache read/write tokens", async () => {
    const { port } = recordingPort(
      makeResponse({
        usage: {
          input_tokens: 12,
          output_tokens: 8,
          cache_read_input_tokens: 2048,
          cache_creation_input_tokens: 512
        }
      })
    );
    const adapter = new AnthropicProviderAdapter({ port });

    const result = await adapter.complete(baseRequest());

    expect(result.usage).toEqual({
      promptTokens: 12,
      completionTokens: 8,
      cacheReadTokens: 2048,
      cacheWriteTokens: 512
    });
    expect(result.content).toBe("hello");
    expect(result.provider).toBe("anthropic");
  });

  it("treats null cache usage as zero", async () => {
    const { port } = recordingPort(
      makeResponse({
        usage: {
          input_tokens: 5,
          output_tokens: 5,
          cache_read_input_tokens: null,
          cache_creation_input_tokens: null
        }
      })
    );
    const adapter = new AnthropicProviderAdapter({ port });

    const result = await adapter.complete(baseRequest());

    expect(result.usage.cacheReadTokens).toBe(0);
    expect(result.usage.cacheWriteTokens).toBe(0);
  });

  it("falls back to the default model when the request model is a placeholder", async () => {
    const { port, calls } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port, defaultModel: "claude-opus-4-8" });

    await adapter.complete(baseRequest({ model: "react-agent" }));

    expect(calls[0]!.model).toBe("claude-opus-4-8");
  });

  it("keeps an explicit Claude model and folds system messages into the system prefix", async () => {
    const { port, calls } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port });

    await adapter.complete(
      baseRequest({
        model: "claude-haiku-4-5",
        system: "Base.",
        messages: [
          { role: "system", content: "Extra rule." },
          { role: "user", content: "Go" }
        ]
      })
    );

    const params = calls[0]!;
    expect(params.model).toBe("claude-haiku-4-5");
    expect(params.system?.[0]?.text).toBe("Base.\n\nExtra rule.");
    // System-role messages are pulled out of the message list.
    expect(params.messages).toEqual([{ role: "user", content: "Go" }]);
  });

  it("surfaces tool_use blocks as structured toolCalls and the stop reason", async () => {
    const { port } = recordingPort(
      makeResponse({
        stop_reason: "tool_use",
        content: [
          { type: "text", text: "Let me search." },
          { type: "tool_use", id: "tu_1", name: "search", input: { query: "ailu" } }
        ]
      })
    );
    const adapter = new AnthropicProviderAdapter({ port });

    const response = await adapter.complete(baseRequest());

    expect(response.content).toBe("Let me search.");
    expect(response.stopReason).toBe("tool_use");
    expect(response.toolCalls).toEqual([{ id: "tu_1", name: "search", input: { query: "ailu" } }]);
  });

  it("passes structured block content (tool_use / tool_result) through to the request", async () => {
    const { port, calls } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port });

    await adapter.complete(
      baseRequest({
        messages: [
          { role: "user", content: "Quelle météo ?" },
          { role: "assistant", content: [{ type: "tool_use", id: "tu1", name: "weather", input: { city: "Paris" } }] },
          {
            role: "user",
            content: [{ type: "tool_result", toolUseId: "tu1", content: '{"temperature":21}' }]
          }
        ]
      })
    );

    const sent = calls[0]!.messages;
    expect(sent).toHaveLength(3);
    expect(Array.isArray(sent[1]!.content)).toBe(true);
    expect(Array.isArray(sent[2]!.content)).toBe(true);
  });

  it("omits toolCalls when the model returns only text", async () => {
    const { port } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port });

    const response = await adapter.complete(baseRequest());

    expect(response.toolCalls).toBeUndefined();
  });

  it("streams text deltas then a terminal chunk", async () => {
    const { port } = recordingPort(makeResponse());
    const adapter = new AnthropicProviderAdapter({ port });

    const chunks: LLMStreamChunk[] = [];
    for await (const chunk of adapter.stream(baseRequest())) {
      chunks.push(chunk);
    }

    expect(chunks.map((c) => c.delta).join("")).toBe("hello");
    expect(chunks.at(-1)?.done).toBe(true);
  });
});
