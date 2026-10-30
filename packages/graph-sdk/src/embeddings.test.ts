import { createServer, type Server } from "node:http";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  createEmbeddings,
  CustomEmbeddingsKeyRequiredError,
  EmbeddingsResponseError,
  MissingEmbeddingsKeyError,
  type EmbeddingsRequestBody
} from "./embeddings.js";

describe("@ailu-ai/graph-sdk — createEmbeddings (injected transport, offline)", () => {
  it("returns the parsed vectors from an injected transport, preserving order", async () => {
    const seen: EmbeddingsRequestBody[] = [];
    const embeddings = createEmbeddings({
      transport: (body) => {
        seen.push(body);
        return {
          data: body.input.map((text, i) => ({ embedding: [text.length, i] }))
        };
      }
    });

    const vectors = await embeddings.embed(["a", "bb", "ccc"]);
    expect(vectors).toEqual([
      [1, 0],
      [2, 1],
      [3, 2]
    ]);
    // Default model + the texts as `input`.
    expect(seen).toEqual([{ model: "mistral-embed", input: ["a", "bb", "ccc"] }]);
  });

  it("honours an injected model and never calls the transport for an empty batch", async () => {
    let called = false;
    const embeddings = createEmbeddings({
      model: "custom-embed",
      transport: (body) => {
        called = true;
        return { data: body.input.map(() => ({ embedding: [0] })) };
      }
    });

    expect(await embeddings.embed([])).toEqual([]);
    expect(called).toBe(false);

    await embeddings.embed(["x"]);
    expect(called).toBe(true);
  });

  it("throws MissingEmbeddingsKeyError when no key and no transport are available", () => {
    const savedKey = process.env.MISTRAL_API_KEY;
    delete process.env.MISTRAL_API_KEY;
    try {
      expect(() => createEmbeddings()).toThrow(MissingEmbeddingsKeyError);
    } finally {
      if (savedKey === undefined) {
        delete process.env.MISTRAL_API_KEY;
      } else {
        process.env.MISTRAL_API_KEY = savedKey;
      }
    }
  });

  it("does not require a key when a transport is injected", () => {
    const savedKey = process.env.MISTRAL_API_KEY;
    delete process.env.MISTRAL_API_KEY;
    try {
      expect(() => createEmbeddings({ transport: () => ({ data: [] }) })).not.toThrow();
    } finally {
      if (savedKey === undefined) {
        delete process.env.MISTRAL_API_KEY;
      } else {
        process.env.MISTRAL_API_KEY = savedKey;
      }
    }
  });

  it("throws EmbeddingsResponseError on a malformed response shape", async () => {
    const embeddings = createEmbeddings({ transport: () => ({ notData: true }) });
    await expect(embeddings.embed(["x"])).rejects.toBeInstanceOf(EmbeddingsResponseError);
  });
});

describe.each([
  { provider: "mistral" as const, envVar: "MISTRAL_API_KEY", baseUrl: "https://api.mistral.ai/v1" },
  { provider: "openai" as const, envVar: "OPENAI_API_KEY", baseUrl: "https://api.openai.com/v1" }
])("embedding credential destination ($provider)", ({ provider, envVar, baseUrl }) => {
  const fetch = vi.fn<typeof globalThis.fetch>();

  beforeEach(() => {
    vi.stubEnv(envVar, "provider-secret-for-test");
    fetch.mockReset();
    fetch.mockResolvedValue(new Response(JSON.stringify({ data: [{ embedding: [1, 2] }] })));
    vi.stubGlobal("fetch", fetch);
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
  });

  it.each([undefined, baseUrl])(
    "uses the environment key at the canonical base URL (%s)",
    async (url) => {
      const embeddings = createEmbeddings({ provider, baseUrl: url, dimensions: 2 });
      expect(await embeddings.embed(["hello"])).toEqual([[1, 2]]);
      expect(fetch).toHaveBeenCalledWith(
        `${baseUrl}/embeddings`,
        expect.objectContaining({
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer provider-secret-for-test"
          },
          body: JSON.stringify({
            model: provider === "mistral" ? "mistral-embed" : "text-embedding-3-small",
            input: ["hello"],
            dimensions: 2
          })
        })
      );
    }
  );

  it.each([
    "https://attacker.example/v1",
    "https://api.openai.com@attacker.example/v1",
    "https://api.mistral.ai.attacker.example/v1",
    "http://api.openai.com/v1",
    "https://api.openai.com:8443/v1",
    "https://api.openai.com/v1?redirect=https://attacker.example",
    "https://api.openai.com/v1#fragment",
    "https://api.openai.com/v1/../other",
    "https://API.OPENAI.COM:443/v1",
    "https://api%2eopenai.com/v1",
    "//attacker.example/v1",
    "not a URL",
    ""
  ])("rejects environment fallback for a custom base URL before I/O: %s", (url) => {
    expect(() => createEmbeddings({ provider, baseUrl: url })).toThrow(
      CustomEmbeddingsKeyRequiredError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("requires an explicit key even for a trailing slash on the default URL", () => {
    expect(() => createEmbeddings({ provider, baseUrl: `${baseUrl}/` })).toThrow(
      CustomEmbeddingsKeyRequiredError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("never shares the selected provider's key with the other provider", () => {
    const other =
      provider === "mistral" ? "https://api.openai.com/v1" : "https://api.mistral.ai/v1";
    expect(() => createEmbeddings({ provider, baseUrl: other })).toThrow(/explicit.*apiKey/);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("uses only the explicit credential for a custom endpoint and captures options at creation", async () => {
    const options = {
      provider,
      baseUrl: "https://custom.example/v1",
      apiKey: "endpoint-secret",
      model: "custom"
    };
    const embeddings = createEmbeddings(options);
    options.baseUrl = baseUrl;
    options.apiKey = "changed";
    vi.stubEnv(envVar, "changed-env");
    expect(await embeddings.embed(["hello"])).toEqual([[1, 2]]);
    expect(fetch).toHaveBeenCalledWith(
      "https://custom.example/v1/embeddings",
      expect.objectContaining({
        headers: { "content-type": "application/json", authorization: "Bearer endpoint-secret" }
      })
    );
  });

  it("rejects an empty explicit key instead of falling back to the environment", () => {
    for (const url of [baseUrl, "https://custom.example/v1"]) {
      expect(() => createEmbeddings({ provider, baseUrl: url, apiKey: "" })).toThrow(
        MissingEmbeddingsKeyError
      );
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it("keeps custom injected transports keyless", async () => {
    const transport = vi.fn(() => ({ data: [{ embedding: [3] }] }));
    const embeddings = createEmbeddings({
      provider,
      baseUrl: "https://custom.example/v1",
      transport
    });
    expect(await embeddings.embed(["hello"])).toEqual([[3]]);
    expect(transport).toHaveBeenCalledOnce();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("refuses redirects in the default bearer transport", async () => {
    await createEmbeddings({ provider }).embed(["hello"]);
    expect(fetch).toHaveBeenCalledWith(
      `${baseUrl}/embeddings`,
      expect.objectContaining({ redirect: "error" })
    );
  });
});

describe("embedding bearer transport redirects", () => {
  const listen = (server: Server): Promise<string> =>
    new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        if (address === null || typeof address === "string")
          return reject(new Error("missing test address"));
        resolve(`http://127.0.0.1:${address.port}`);
      });
    });
  const close = (server: Server): Promise<void> =>
    new Promise((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
      server.closeAllConnections();
    });

  it.each([301, 302, 303, 307, 308])(
    "does not follow a %s redirect to another origin",
    async (status) => {
      const received: (string | undefined)[] = [];
      const target = createServer((request, response) => {
        received.push(request.headers.authorization);
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify({ data: [{ embedding: [1] }] }));
      });
      const targetUrl = await listen(target);
      const source = createServer((_request, response) => {
        response.writeHead(status, { location: `${targetUrl}/embeddings` });
        response.end();
      });
      try {
        const baseUrl = await listen(source);
        await expect(
          createEmbeddings({ baseUrl, apiKey: "endpoint-secret" }).embed(["hello"])
        ).rejects.toThrow();
        expect(received).toEqual([]);
      } finally {
        await Promise.all([close(source), close(target)]);
      }
    }
  );
});

describe("@ailu-ai/graph-sdk — createEmbeddings multi-provider (issue #541)", () => {
  it("defaults to mistral-embed when no provider is given (unchanged behavior)", async () => {
    const seen: EmbeddingsRequestBody[] = [];
    const embeddings = createEmbeddings({
      transport: (body) => {
        seen.push(body);
        return { data: [{ embedding: [1] }] };
      }
    });
    await embeddings.embed(["x"]);
    expect(seen).toEqual([{ model: "mistral-embed", input: ["x"] }]);
  });

  it("resolves the openai provider's own default model, never sending dimensions unless set", async () => {
    const seen: EmbeddingsRequestBody[] = [];
    const embeddings = createEmbeddings({
      provider: "openai",
      transport: (body) => {
        seen.push(body);
        return { data: [{ embedding: [1] }] };
      }
    });
    await embeddings.embed(["x"]);
    expect(seen).toEqual([{ model: "text-embedding-3-small", input: ["x"] }]);
  });

  it("includes dimensions in the request body only when explicitly set", async () => {
    const seen: EmbeddingsRequestBody[] = [];
    const embeddings = createEmbeddings({
      provider: "openai",
      dimensions: 1024,
      transport: (body) => {
        seen.push(body);
        return { data: [{ embedding: new Array(1024).fill(0) }] };
      }
    });
    await embeddings.embed(["x"]);
    expect(seen).toEqual([{ model: "text-embedding-3-small", input: ["x"], dimensions: 1024 }]);
  });

  it("throws MissingEmbeddingsKeyError naming the OPENAI_API_KEY env var for the openai provider", () => {
    const saved = process.env.OPENAI_API_KEY;
    delete process.env.OPENAI_API_KEY;
    try {
      expect(() => createEmbeddings({ provider: "openai" })).toThrow(MissingEmbeddingsKeyError);
      try {
        createEmbeddings({ provider: "openai" });
      } catch (error) {
        expect((error as Error).message).toContain("OPENAI_API_KEY");
      }
    } finally {
      if (saved === undefined) delete process.env.OPENAI_API_KEY;
      else process.env.OPENAI_API_KEY = saved;
    }
  });

  it("an explicit apiKey/model/baseUrl still overrides the provider's own defaults", async () => {
    const seen: EmbeddingsRequestBody[] = [];
    const embeddings = createEmbeddings({
      provider: "openai",
      model: "custom-model",
      transport: (body) => {
        seen.push(body);
        return { data: [{ embedding: [1] }] };
      }
    });
    await embeddings.embed(["x"]);
    expect(seen).toEqual([{ model: "custom-model", input: ["x"] }]);
  });
});
