import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import {
  createGraph,
  InMemoryToolRegistry,
  model,
  runCatalogGraph,
  rustEngineAvailable,
  toRustAgentConfig,
  type AgentNodeConfig,
  type AgentResult
} from "./index.js";

/**
 * Web search by the model provider (ailu-core#284): the agent option, what the engine sends to
 * Mistral's Conversations API, and the sources the agent's result reports.
 */

const searchTools = (): InMemoryToolRegistry => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "search_kb" as never,
      name: "search_kb",
      description: "Search the knowledge base.",
      inputSchema: { parse: (value: unknown) => value as { query: string } },
      outputSchema: { parse: (value: unknown) => value as { hits: string[] } },
      permissions: [],
      jsonSchema: { type: "object", properties: { query: { type: "string" } } }
    },
    async () => ({ hits: [] })
  );
  return tools;
};

const base: AgentNodeConfig = {
  model: model.mistral("mistral-small-latest"),
  prompt: { system: "Answer from the web." }
};

describe("agentNode webSearch (pure)", () => {
  it("defaults maxUses and keeps the domain list", () => {
    const config = toRustAgentConfig("researcher", {
      ...base,
      webSearch: { allowedDomains: ["europa.eu"] }
    });
    expect(config.webSearch).toEqual({ maxUses: 3, allowedDomains: ["europa.eu"] });
    expect(toRustAgentConfig("plain", base).webSearch).toBeUndefined();
  });

  it("refuses what the engine could not honor, instead of dropping the web", () => {
    const invalid: AgentNodeConfig[] = [
      { ...base, tools: searchTools(), webSearch: {} },
      { ...base, enableFs: true, webSearch: {} },
      { ...base, memory: { namespace: "notes" }, webSearch: {} },
      { ...base, webSearch: { allowedDomains: ["a.eu"], blockedDomains: ["b.com"] } },
      { ...base, webSearch: { maxUses: 0 } }
    ];
    for (const config of invalid) {
      expect(() => toRustAgentConfig("researcher", config)).toThrowError(
        expect.objectContaining({ code: "AILU_WEB_SEARCH_INVALID" })
      );
    }
  });

  it("is carried on the saved definition", () => {
    const app = createGraph({ name: "web-carrier" })
      .agentNode("researcher", { ...base, webSearch: { maxUses: 2 } })
      .compile();
    const node = app.definition.nodes.find((candidate) => candidate.id === "researcher");
    const carrier = (node?.metadata as { agent?: { webSearch?: unknown } } | undefined)?.agent;
    expect(carrier?.webSearch).toEqual({ maxUses: 2 });
  });
});

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

/** The Conversations response for one `web_search` call. */
const searchedConversation = {
  conversation_id: "conv_0",
  object: "conversation.response",
  outputs: [
    {
      type: "tool.execution",
      object: "entry",
      name: "web_search",
      id: "tool_exec_0",
      arguments: JSON.stringify({ query: "cohere eu hosting" })
    },
    {
      type: "message.output",
      object: "entry",
      role: "assistant",
      id: "msg_0",
      content: [
        { type: "text", text: "Cohere hosts private deployments in the EU." },
        {
          type: "tool_reference",
          tool: "web_search",
          title: "Cohere EU data residency",
          url: "https://cohere.com/eu",
          source: "brave"
        }
      ]
    }
  ],
  usage: {
    prompt_tokens: 40,
    completion_tokens: 12,
    total_tokens: 900,
    connector_tokens: 848,
    connectors: { web_search: 1 }
  }
};

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("agentNode webSearch on the Rust engine (Mistral Conversations)", () => {
  const seen: SeenRequest[] = [];
  let server: Server;
  let baseURL = "";
  const savedKey = process.env.TEST_MISTRAL_KEY;

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((raw) => {
        seen.push({ url: request.url, body: JSON.parse(raw) as Record<string, unknown> });
        if (request.url?.endsWith("/conversations") === true) {
          response.writeHead(200, { "content-type": "application/json" });
          response.end(JSON.stringify(searchedConversation));
        } else {
          response.writeHead(404);
          response.end();
        }
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
    process.env.TEST_MISTRAL_KEY = "test-mistral-key";
  });

  afterEach(() => {
    if (savedKey === undefined) {
      delete process.env.TEST_MISTRAL_KEY;
    } else {
      process.env.TEST_MISTRAL_KEY = savedKey;
    }
  });

  const webAgent = (): AgentNodeConfig => ({
    model: model.mistral("mistral-small-latest", { baseURL, apiKeyEnv: "TEST_MISTRAL_KEY" }),
    prompt: { system: "Answer from the web." },
    webSearch: { maxUses: 2 },
    maxIterations: 1
  });

  const expectedWebSearch = {
    sources: [{ url: "https://cohere.com/eu", title: "Cohere EU data residency", cited: true }],
    requests: 1,
    queries: ["cohere eu hosting"]
  };

  it("asks Mistral with its web_search connector, keeps nothing, and reports the sources", async () => {
    const app = createGraph({ name: "web" }).agentNode("researcher", webAgent()).compile();

    const result = await app.run({ question: "Where does Cohere host in the EU?" });

    expect(result.status).toBe("completed");
    expect(seen).toHaveLength(1);
    expect(seen[0]?.url).toBe("/v1/conversations");
    const body = seen[0]?.body ?? {};
    expect(body.tools).toEqual([{ type: "web_search" }]);
    expect(body.store).toBe(false);
    expect(body.model).toBe("mistral-small-latest");
    expect(String(body.instructions)).toContain("Answer from the web.");
    const agent = result.channels.agentResult as AgentResult;
    expect(agent.webSearch).toEqual(expectedWebSearch);
  });

  it("runs the same web search from the saved definition (runCatalogGraph)", async () => {
    const app = createGraph({ name: "web-catalog" }).agentNode("researcher", webAgent()).compile();

    const outcome = await runCatalogGraph(app.definition, {
      initialData: { question: "Where does Cohere host in the EU?" }
    });

    expect(outcome.status).toBe("completed");
    expect(seen[0]?.url).toBe("/v1/conversations");
    const agent = outcome.state.channels.agentResult as AgentResult;
    expect(agent.webSearch).toEqual(expectedWebSearch);
  });
});
