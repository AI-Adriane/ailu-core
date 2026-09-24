import { describe, expect, it } from "vitest";

import {
  createEmbeddings,
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
      expect(() =>
        createEmbeddings({ transport: () => ({ data: [] }) })
      ).not.toThrow();
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
