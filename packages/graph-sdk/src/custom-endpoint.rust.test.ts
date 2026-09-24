import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { createGraph, model, rustEngineAvailable } from "./index.js";

/**
 * `model.openaiCompatible({ baseURL })` must reach THAT endpoint — on the graph path (agent node
 * in the Rust engine) and on `invoke()` — with the key named by `apiKeyEnv` only, never the
 * provider's public-API key.
 */

type SeenRequest = { url: string | undefined; authorization: string | undefined };

const readBody = (request: IncomingMessage): Promise<string> =>
  new Promise((resolve, reject) => {
    let body = "";
    request.on("data", (chunk: Buffer) => {
      body += chunk.toString("utf8");
    });
    request.on("end", () => resolve(body));
    request.on("error", reject);
  });

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("custom OpenAI-compatible endpoint (baseURL)", () => {
  const seen: SeenRequest[] = [];
  let server: Server;
  let baseURL = "";
  const savedEnv: Record<string, string | undefined> = {};

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then(() => {
        seen.push({ url: request.url, authorization: request.headers.authorization });
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify({
            model: "llama-3",
            choices: [
              { message: { role: "assistant", content: "served by the custom endpoint" }, finish_reason: "stop" }
            ],
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
    for (const key of ["OPENAI_API_KEY", "ADRIANE_TEST_ENDPOINT_KEY"]) {
      savedEnv[key] = process.env[key];
    }
    // A public OpenAI key is present: it must never be sent to the custom endpoint.
    process.env.OPENAI_API_KEY = "sk-public-openai";
    process.env.ADRIANE_TEST_ENDPOINT_KEY = "endpoint-secret";
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

  it("an agent node sends its request to the baseURL with the apiKeyEnv key", async () => {
    const app = createGraph({ name: "custom-endpoint-agent" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL, model: "llama-3", apiKeyEnv: "ADRIANE_TEST_ENDPOINT_KEY" }),
        prompt: { system: "Be brief." },
        maxIterations: 1
      })
      .compile();

    const result = await app.run({ question: "hi" });

    expect(result.status).toBe("completed");
    expect(seen).toEqual([{ url: "/v1/chat/completions", authorization: "Bearer endpoint-secret" }]);
  });

  it("invoke() sends its request to the baseURL, keyless when no apiKeyEnv is named", async () => {
    const response = await model.openaiCompatible({ baseURL, model: "llama-3" }).invoke("hi");

    expect(response.content).toBe("served by the custom endpoint");
    expect(seen).toEqual([{ url: "/v1/chat/completions", authorization: undefined }]);
  });
});
