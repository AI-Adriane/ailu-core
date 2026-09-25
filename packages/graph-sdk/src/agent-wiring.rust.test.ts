import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import {
  council,
  createGraph,
  InMemoryToolRegistry,
  model,
  runCatalogGraph,
  rustEngineAvailable,
  toRustAgentConfig,
  type AgentResult
} from "./index.js";

/**
 * What an agent node sends to the engine: the tools the LLM is shown, a missing key failing loud,
 * a `"provider:model"` string, and the persisted carrier that lets `runCatalogGraph` run the same
 * graph (custom endpoint and `mapAgents` included).
 */

type SeenRequest = { url: string | undefined; body: Record<string, unknown> };

const readBody = (request: IncomingMessage): Promise<string> =>
  new Promise((resolve, reject) => {
    let body = "";
    request.on("data", (chunk: Buffer) => {
      body += chunk.toString("utf8");
    });
    request.on("end", () => resolve(body));
    request.on("error", reject);
  });

const refundTools = (): InMemoryToolRegistry => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "refund" as never,
      name: "refund",
      description: "Refund an order by its id.",
      inputSchema: { parse: (value: unknown) => value as { orderId: string } },
      outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
      permissions: [],
      jsonSchema: {
        type: "object",
        properties: { orderId: { type: "string" } },
        required: ["orderId"]
      }
    },
    async () => ({ ok: true })
  );
  return tools;
};

describe("agentNode wiring (pure)", () => {
  it('parses a "provider:model" string like model("provider:model")', () => {
    const config = toRustAgentConfig("a", { model: "openai:gpt-4o", prompt: { system: "hi" } });
    expect(config.provider).toBe("openai");
    expect(config.model).toBe("gpt-4o");

    const tier = toRustAgentConfig("a", { model: "mistral:fast", prompt: { system: "hi" } });
    expect(tier.provider).toBe("mistral");
    expect(tier.tier).toBe("fast");
    expect(tier.model).toBeUndefined();
  });

  it("rejects an unknown provider in a model string instead of running on another provider", () => {
    expect(() => toRustAgentConfig("a", { model: "groq:llama-3", prompt: { system: "hi" } })).toThrow(
      /Unknown provider "groq"/
    );
    // The legacy flat `provider` field is checked the same way.
    expect(() =>
      toRustAgentConfig("a", { provider: "groq" as never, model: "llama-3", prompt: { system: "hi" } })
    ).toThrow(/Unknown provider "groq"/);
  });

  it("keeps a named provider binding with a tier, and leaves a tier-only model's provider blank", () => {
    const named = toRustAgentConfig("a", { model: model.anthropic.frontier, prompt: { system: "hi" } });
    expect(named.provider).toBe("anthropic");
    expect(named.tier).toBe("frontier");
    const tierOnly = toRustAgentConfig("a", { model: model.fast, prompt: { system: "hi" } });
    expect(tierOnly.provider).toBe("");
    expect(tierOnly.tier).toBe("fast");
  });

  it("keeps a bare legacy model id as the model", () => {
    const config = toRustAgentConfig("a", { model: "claude-sonnet-4-6", prompt: { system: "hi" } });
    expect(config.provider).toBe("anthropic");
    expect(config.model).toBe("claude-sonnet-4-6");
  });

  it("carries each tool's description and JSON Schema", () => {
    const config = toRustAgentConfig("a", { tools: refundTools(), prompt: { system: "hi" } });
    expect(config.toolSpecs).toEqual([
      {
        name: "refund",
        description: "Refund an order by its id.",
        jsonSchema: {
          type: "object",
          properties: { orderId: { type: "string" } },
          required: ["orderId"]
        }
      }
    ]);
  });

  it("persists the custom endpoint and the tool specs on the agent carrier", () => {
    const app = createGraph({ name: "carrier" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL: "http://127.0.0.1:1/v1", model: "llama-3", apiKeyEnv: "MY_KEY" }),
        tools: refundTools(),
        prompt: { system: "hi" }
      })
      .compile();
    const agent = app.definition.nodes.find((node) => node.id === "reply")?.metadata?.agent as Record<
      string,
      unknown
    >;
    expect(agent.baseURL).toBe("http://127.0.0.1:1/v1");
    expect(agent.apiKeyEnv).toBe("MY_KEY");
    expect(agent.toolSpecs).toHaveLength(1);
    expect(agent.toolBindings).toBeUndefined();
  });
});

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("agentNode wiring on the Rust engine", () => {
  const seen: SeenRequest[] = [];
  let server: Server;
  let baseURL = "";
  const savedEnv: Record<string, string | undefined> = {};

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((body) => {
        seen.push({ url: request.url, body: JSON.parse(body) as Record<string, unknown> });
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify({
            model: "llama-3",
            choices: [{ message: { role: "assistant", content: "FINAL: ok" }, finish_reason: "stop" }],
            usage: { prompt_tokens: 1, completion_tokens: 1 }
          })
        );
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    baseURL = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  beforeEach(() => {
    seen.length = 0;
    for (const key of ["AILU_LLM_MOCK", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"]) {
      savedEnv[key] = process.env[key];
    }
  });

  afterEach(() => {
    for (const [key, value] of Object.entries(savedEnv)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  });

  it("shows the LLM each tool's description and input schema", async () => {
    const app = createGraph({ name: "tool-advertising" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        tools: refundTools(),
        prompt: { system: "Be brief." },
        maxIterations: 1
      })
      .compile();

    const result = await app.run({ question: "refund order 42" });

    expect(result.status).toBe("completed");
    const tools = seen[0]?.body.tools as Array<{ function: Record<string, unknown> }>;
    expect(tools[0]?.function).toMatchObject({
      name: "refund",
      description: "Refund an order by its id.",
      parameters: {
        type: "object",
        properties: { orderId: { type: "string" } },
        required: ["orderId"]
      }
    });
  });

  it("fails loud with no API key, and runs on the mock only in offline mode", async () => {
    delete process.env.AILU_LLM_MOCK;
    delete process.env.ANTHROPIC_API_KEY;
    const app = createGraph({ name: "keyless" })
      .agentNode("reply", { model: model.anthropic("claude-sonnet-4-6"), prompt: { system: "Be brief." } })
      .compile();

    const failed = await app.run({ question: "hi" }).then(
      (outcome) => JSON.stringify(outcome),
      (error: unknown) => String(error)
    );
    expect(failed).toMatch(/ANTHROPIC_API_KEY/);
    expect(failed).toMatch(/AILU_LLM_MOCK=1/);

    process.env.AILU_LLM_MOCK = "1";
    const offline = await app.run({ question: "hi" });
    expect(offline.status).toBe("completed");
    expect((offline.channels.agentResult as AgentResult).reasoning).toContain("done");
  });

  it("runCatalogGraph keeps the custom endpoint of a builder graph", async () => {
    // A public OpenAI key is present: the saved graph must still go to its own endpoint.
    process.env.OPENAI_API_KEY = "sk-public-openai";
    const app = createGraph({ name: "catalog-endpoint" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        prompt: { system: "Be brief." },
        maxIterations: 1
      })
      .compile();

    const outcome = await runCatalogGraph(app.definition, { initialData: { question: "hi" } });

    expect(outcome.status).toBe("completed");
    expect(seen.map((request) => request.url)).toEqual(["/v1/chat/completions"]);
  });

  it("shows an agent only its visibleChannels", async () => {
    const app = createGraph({ name: "isolation" })
      .channel("question", { type: "string", default: "" })
      .channel("secret", { type: "string", default: "" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        prompt: { system: "Be brief." },
        visibleChannels: ["question"],
        maxIterations: 1
      })
      .compile();

    await app.run({ question: "what is the plan?", secret: "do-not-show" });

    const sent = JSON.stringify(seen[0]?.body.messages);
    expect(sent).toContain("what is the plan?");
    expect(sent).not.toContain("do-not-show");
  });

  it("council reviewers rank the field without seeing who wrote each answer", async () => {
    const seat = { model: model.openaiCompatible({ baseURL, model: "llama-3" }), prompt: { system: "Answer." } };
    const definition = council({ members: [seat, seat], chair: seat });

    const outcome = await runCatalogGraph(definition, { initialData: { query: "which plan?" } });

    expect(outcome.status).toBe("completed");
    // 2 members, 2 reviewers, 1 chair. Reviewers are the 3rd and 4th requests.
    expect(seen).toHaveLength(5);
    for (const reviewer of seen.slice(2, 4)) {
      const sent = JSON.stringify(reviewer.body.messages);
      expect(sent).toContain("which plan?");
      expect(sent).not.toMatch(/member_\d|memberId/);
    }
    expect(outcome.state.channels.fieldKey).toHaveLength(2);
  });

  it("runCatalogGraph fans out a builder mapAgents node", async () => {
    const app = createGraph({ name: "catalog-map" })
      .channel("items", { type: "json", default: [] as string[] })
      .mapAgents("fanout", {
        overChannel: "items",
        subAgent: { model: model.openaiCompatible({ baseURL, model: "llama-3" }), prompt: { system: "Summarise." } },
        joinAt: "summaries"
      })
      .compile();

    const outcome = await runCatalogGraph(app.definition, { initialData: { items: ["a", "b", "c"] } });

    expect(outcome.status).toBe("completed");
    expect(outcome.state.channels.summaries).toHaveLength(3);
    expect(seen).toHaveLength(3);
  });
});
