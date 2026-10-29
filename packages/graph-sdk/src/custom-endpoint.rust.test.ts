import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { ApiKeyEnvNotAllowedError, createGraph, model, rustEngineAvailable } from "./index.js";

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
  let redirectTo: string | undefined;
  const savedEnv: Record<string, string | undefined> = {};

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then(() => {
        seen.push({ url: request.url, authorization: request.headers.authorization });
        if (redirectTo !== undefined) {
          response.writeHead(307, { location: redirectTo });
          response.end();
          return;
        }
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify({
            model: "llama-3",
            choices: [
              {
                message: { role: "assistant", content: "served by the custom endpoint" },
                finish_reason: "stop"
              }
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
    redirectTo = undefined;
    for (const key of [
      "OPENAI_API_KEY",
      "AILU_TEST_ENDPOINT_KEY",
      "AILU_API_KEY_ENV_ALLOWLIST",
      "AILU_MODEL_ENDPOINT_ALLOWLIST"
    ]) {
      savedEnv[key] = process.env[key];
    }
    // A public OpenAI key is present: it must never be sent to the custom endpoint.
    process.env.OPENAI_API_KEY = "sk-public-openai";
    process.env.AILU_TEST_ENDPOINT_KEY = "endpoint-secret";
    process.env.AILU_MODEL_ENDPOINT_ALLOWLIST = baseURL;
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
        model: model.openaiCompatible({
          baseURL,
          model: "llama-3",
          apiKeyEnv: "AILU_TEST_ENDPOINT_KEY"
        }),
        prompt: { system: "Be brief." },
        maxIterations: 1
      })
      .compile();

    const result = await app.run({ question: "hi" });

    expect(result.status).toBe("completed");
    expect(seen).toEqual([
      { url: "/v1/chat/completions", authorization: "Bearer endpoint-secret" }
    ]);
  });

  it("an agent whose apiKeyEnv is refused throws ApiKeyEnvNotAllowedError", async () => {
    process.env.AILU_API_KEY_ENV_ALLOWLIST = "AILU_TEST_ALLOWED_*";
    const app = createGraph({ name: "custom-endpoint-refused" })
      .agentNode("reply", {
        model: model.openaiCompatible({
          baseURL,
          model: "llama-3",
          apiKeyEnv: "AILU_TEST_ENDPOINT_KEY"
        }),
        prompt: { system: "Be brief." },
        maxIterations: 1
      })
      .compile();

    const failure: unknown = await app.run({ question: "hi" }).then(
      () => undefined,
      (error: unknown) => error
    );

    expect(failure).toBeInstanceOf(ApiKeyEnvNotAllowedError);
    const refused = failure as ApiKeyEnvNotAllowedError;
    expect(refused.code).toBe("AILU_API_KEY_ENV_NOT_ALLOWED");
    expect(refused.envVar).toBe("AILU_TEST_ENDPOINT_KEY");
    expect(refused.message).toContain("agent node 'reply'");
    expect(refused.message).not.toContain("endpoint-secret");
    expect(seen).toEqual([]);
  });

  it("invoke() sends its request to the baseURL, keyless when no apiKeyEnv is named", async () => {
    const response = await model.openaiCompatible({ baseURL, model: "llama-3" }).invoke("hi");

    expect(response.content).toBe("served by the custom endpoint");
    expect(seen).toEqual([{ url: "/v1/chat/completions", authorization: undefined }]);
  });

  it("denies graph and standalone custom destinations before sending a request", async () => {
    delete process.env.AILU_MODEL_ENDPOINT_ALLOWLIST;
    const endpoint = model.openaiCompatible({ baseURL, model: "llama-3" });
    const app = createGraph({ name: "custom-endpoint-denied" })
      .agentNode("reply", { model: endpoint, prompt: { system: "Be brief." }, maxIterations: 1 })
      .compile();

    await expect(app.run({ question: "hi" })).rejects.toThrow("AILU_MODEL_ENDPOINT_NOT_ALLOWED");
    await expect(endpoint.invoke("hi")).rejects.toThrow("AILU_MODEL_ENDPOINT_NOT_ALLOWED");
    expect(seen).toEqual([]);
  });

  it("does not follow an authorized endpoint's redirect to an unauthorized path", async () => {
    redirectTo = `${baseURL}/forbidden`;
    await expect(
      model.openaiCompatible({ baseURL, model: "llama-3" }).invoke("hi")
    ).rejects.toThrow("307");
    expect(seen).toEqual([{ url: "/v1/chat/completions", authorization: undefined }]);
  });
});
