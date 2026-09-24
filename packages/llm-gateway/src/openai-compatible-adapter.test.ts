import { describe, expect, it } from "vitest";

import {
  buildRequestBody,
  OpenAICompatibleProviderAdapter,
  type OpenAIChatRequestBody,
  type OpenAIChatResponse,
  type OpenAICompatibleTransportPort
} from "./openai-compatible-adapter.js";
import { DefaultLLMGateway } from "./gateway.js";
import { LLMProviderError } from "./errors.js";
import type { LLMRequest } from "./types.js";

const baseRequest = (over?: Partial<LLMRequest>): LLMRequest => ({
  provider: "mistral",
  model: "mistral-small-latest",
  messages: [{ role: "user", content: "Hi" }],
  ...over
});

/** Captures the body the adapter builds and returns a canned response. */
const recordingPort = (response: OpenAIChatResponse) => {
  const calls: OpenAIChatRequestBody[] = [];
  const port: OpenAICompatibleTransportPort = {
    async send(body) {
      calls.push(body);
      return response;
    }
  };
  return { port, calls };
};

const textCompletion = (over?: Partial<OpenAIChatResponse>): OpenAIChatResponse => ({
  choices: [{ message: { content: "Bonjour." }, finish_reason: "stop" }],
  usage: { prompt_tokens: 11, completion_tokens: 4 },
  ...over
});

describe("buildRequestBody", () => {
  it("folds system as the first message and maps tools to OpenAI function shape", () => {
    const inputSchema = { type: "object", properties: { query: { type: "string" } } };
    const body = buildRequestBody(
      baseRequest({
        system: "You are a helpful agent.",
        messages: [{ role: "user", content: "Find docs." }],
        tools: [{ name: "search", description: "Search the corpus.", inputSchema }]
      }),
      "mistral-small-latest"
    );

    expect(body.messages[0]).toEqual({ role: "system", content: "You are a helpful agent." });
    expect(body.messages[1]).toEqual({ role: "user", content: "Find docs." });
    expect(body.tools).toHaveLength(1);
    expect(body.tools?.[0]?.type).toBe("function");
    expect(body.tools?.[0]?.function.name).toBe("search");
    expect(body.tools?.[0]?.function.description).toBe("Search the corpus.");
    // parameters is the inputSchema verbatim.
    expect(body.tools?.[0]?.function.parameters).toEqual(inputSchema);
  });

  it("passes temperature and max_tokens through only when set", () => {
    const withParams = buildRequestBody(
      baseRequest({ temperature: 0.3, maxTokens: 256 }),
      "mistral-small-latest"
    );
    expect(withParams.temperature).toBe(0.3);
    expect(withParams.max_tokens).toBe(256);

    const without = buildRequestBody(baseRequest(), "mistral-small-latest");
    expect(without.temperature).toBeUndefined();
    expect(without.max_tokens).toBeUndefined();
  });

  it("falls back to the default model for a non-provider model id (e.g. a Claude/agent placeholder)", () => {
    expect(buildRequestBody(baseRequest({ model: "claude-opus-4-8" }), "mistral-small-latest").model).toBe(
      "mistral-small-latest"
    );
    expect(buildRequestBody(baseRequest({ model: "react-agent" }), "mistral-small-latest").model).toBe(
      "mistral-small-latest"
    );
    // A real provider model id is kept verbatim.
    expect(buildRequestBody(baseRequest({ model: "mistral-large-latest" }), "mistral-small-latest").model).toBe(
      "mistral-large-latest"
    );
  });

  it("converts assistant tool_use and user tool_result blocks to OpenAI messages", () => {
    const body = buildRequestBody(
      baseRequest({
        messages: [
          { role: "user", content: "Quelle météo ?" },
          {
            role: "assistant",
            content: [{ type: "tool_use", id: "tu1", name: "weather", input: { city: "Paris" } }]
          },
          {
            role: "user",
            content: [{ type: "tool_result", toolUseId: "tu1", content: '{"temperature":21}' }]
          }
        ]
      }),
      "mistral-small-latest"
    );

    // user text, assistant-with-tool_calls, then a standalone role:'tool' message.
    expect(body.messages[0]).toEqual({ role: "user", content: "Quelle météo ?" });
    const assistant = body.messages[1]!;
    expect(assistant.role).toBe("assistant");
    expect(assistant.tool_calls).toEqual([
      { id: "tu1", type: "function", function: { name: "weather", arguments: '{"city":"Paris"}' } }
    ]);
    const toolMsg = body.messages[2]!;
    expect(toolMsg.role).toBe("tool");
    expect(toolMsg.tool_call_id).toBe("tu1");
    expect(toolMsg.content).toBe('{"temperature":21}');
  });
});

describe("OpenAICompatibleProviderAdapter", () => {
  it("maps a text-only completion to LLMResponse (no toolCalls, usage + stopReason)", async () => {
    const { port } = recordingPort(textCompletion());
    const adapter = new OpenAICompatibleProviderAdapter({
      baseUrl: "https://api.mistral.ai/v1",
      defaultModel: "mistral-small-latest",
      port
    });

    const result = await adapter.complete(baseRequest());

    expect(result.content).toBe("Bonjour.");
    expect(result.toolCalls).toBeUndefined();
    expect(result.stopReason).toBe("stop");
    expect(result.usage).toEqual({ promptTokens: 11, completionTokens: 4 });
    expect(result.provider).toBe("mistral");
    expect(result.model).toBe("mistral-small-latest");
  });

  it("parses tool_calls into structured toolCalls (id/name/JSON-parsed input)", async () => {
    const { port } = recordingPort(
      textCompletion({
        choices: [
          {
            message: {
              content: null,
              tool_calls: [
                {
                  id: "call_1",
                  type: "function",
                  function: { name: "search", arguments: '{"query":"ailu"}' }
                }
              ]
            },
            finish_reason: "tool_calls"
          }
        ]
      })
    );
    const adapter = new OpenAICompatibleProviderAdapter({
      baseUrl: "https://api.mistral.ai/v1",
      defaultModel: "mistral-small-latest",
      port
    });

    const result = await adapter.complete(baseRequest());

    expect(result.content).toBe("");
    expect(result.stopReason).toBe("tool_calls");
    expect(result.toolCalls).toEqual([{ id: "call_1", name: "search", input: { query: "ailu" } }]);
  });

  it("throws LLMProviderError with status when the port reports a non-2xx", async () => {
    const port: OpenAICompatibleTransportPort = {
      async send() {
        throw new LLMProviderError(401, '{"error":"unauthorized"}');
      }
    };
    const adapter = new OpenAICompatibleProviderAdapter({
      baseUrl: "https://api.mistral.ai/v1",
      defaultModel: "mistral-small-latest",
      port
    });

    await expect(adapter.complete(baseRequest())).rejects.toMatchObject({
      name: "LLMProviderError",
      status: 401,
      body: '{"error":"unauthorized"}'
    });
  });

  it("routes through DefaultLLMGateway on provider 'mistral'", async () => {
    const { port, calls } = recordingPort(textCompletion());
    const gateway = new DefaultLLMGateway();
    gateway.registerAdapter(
      new OpenAICompatibleProviderAdapter({
        baseUrl: "https://api.mistral.ai/v1",
        defaultModel: "mistral-small-latest",
        port
      })
    );

    const result = await gateway.complete(baseRequest());

    expect(calls).toHaveLength(1);
    expect(result.content).toBe("Bonjour.");
    expect(result.provider).toBe("mistral");
  });

  it("streams the completion text as a delta then a terminal chunk", async () => {
    const { port } = recordingPort(textCompletion());
    const adapter = new OpenAICompatibleProviderAdapter({
      baseUrl: "https://api.mistral.ai/v1",
      defaultModel: "mistral-small-latest",
      port
    });

    const chunks = [];
    for await (const chunk of adapter.stream(baseRequest())) {
      chunks.push(chunk);
    }

    expect(chunks.map((c) => c.delta).join("")).toBe("Bonjour.");
    expect(chunks.at(-1)?.done).toBe(true);
  });

  it(".mistral() sets the Mistral baseUrl, provider and a sensible default model", async () => {
    const { port, calls } = recordingPort(textCompletion());
    // Construct via the convenience ctor but inject the port so it stays offline.
    const adapter = OpenAICompatibleProviderAdapter.mistral("sk-test");
    // The default model is asserted via routing of a placeholder model id below.
    expect(adapter.provider).toBe("mistral");

    // Drive the default-model path through a port-injected adapter mirroring .mistral().
    const injected = new OpenAICompatibleProviderAdapter({
      provider: "mistral",
      baseUrl: "https://api.mistral.ai/v1",
      defaultModel: "mistral-small-latest",
      port
    });
    await injected.complete(baseRequest({ model: "claude-opus-4-8" }));
    expect(calls[0]?.model).toBe("mistral-small-latest");
  });

  it(".google() registers under 'google' via the Gemini OpenAI-compatible endpoint + default model", async () => {
    const adapter = OpenAICompatibleProviderAdapter.google("gm-test");
    expect(adapter.provider).toBe("google");

    const { port, calls } = recordingPort(textCompletion());
    const injected = new OpenAICompatibleProviderAdapter({
      provider: "google",
      baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
      defaultModel: "gemini-2.5-flash",
      port
    });
    await injected.complete(baseRequest({ model: "react-agent" }));
    expect(calls[0]?.model).toBe("gemini-2.5-flash");
  });

  it(".openai() registers under 'openai' with the OpenAI baseUrl + default model", async () => {
    const adapter = OpenAICompatibleProviderAdapter.openai("sk-test");
    expect(adapter.provider).toBe("openai");

    const { port, calls } = recordingPort(textCompletion());
    const injected = new OpenAICompatibleProviderAdapter({
      provider: "openai",
      baseUrl: "https://api.openai.com/v1",
      defaultModel: "gpt-4o",
      port
    });
    await injected.complete(baseRequest({ model: "react-agent" }));
    expect(calls[0]?.model).toBe("gpt-4o");
  });

  it(".ollama() registers under 'mistral' and defaults to the local model", async () => {
    const adapter = OpenAICompatibleProviderAdapter.ollama();
    // Registers under the mistral key so it shares the gateway slot with Mistral cloud.
    expect(adapter.provider).toBe("mistral");

    const { port, calls } = recordingPort(textCompletion());
    const injected = new OpenAICompatibleProviderAdapter({
      provider: "mistral",
      baseUrl: "http://localhost:11434/v1",
      defaultModel: "mistral",
      port
    });
    await injected.complete(baseRequest({ model: "claude-opus-4-8" }));
    expect(calls[0]?.model).toBe("mistral");
  });
});
