import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  cosineSimilarity,
  createEmbeddings,
  createVectorStore,
  rustEngineAvailable
} from "./index.js";

/**
 * ADR 0045 M4 — what the TypeScript embeddings and vector store do, as golden data for the
 * engine's port (`crates/llm-gateway` embeddings, `crates/runtime-bridge` vector query) that the
 * Python SDK calls: the request body `createEmbeddings` sends, how it reads a response (vectors or
 * the error), and what `createVectorStore().query` and `cosineSimilarity` return.
 * `AILU_UPDATE_EMBEDDINGS_GOLDEN=1` rewrites the file from this SDK.
 */
const FIXTURE = fileURLToPath(
  new URL(
    "../../../crates/runtime-bridge/tests/fixtures/embeddings_vectors_golden.json",
    import.meta.url
  )
);

type BodyCase = { name: string; options: Record<string, unknown>; texts: string[] };
type ResponseCase = { name: string; response: unknown };
type QueryCase = { name: string; items: unknown[]; embedding: number[]; k: number };
type CosineCase = { name: string; a: number[]; b: number[] };

const bodyCases: BodyCase[] = [
  { name: "mistral defaults", options: {}, texts: ["a", "b"] },
  { name: "openai defaults", options: { provider: "openai" }, texts: ["x"] },
  {
    name: "a model and dimensions",
    options: { provider: "openai", model: "text-embedding-3-large", dimensions: 256 },
    texts: ["x"]
  },
  { name: "no texts: no call", options: {}, texts: [] }
];

const responseCases: ResponseCase[] = [
  {
    name: "two vectors",
    response: { data: [{ embedding: [0.1, 0.2] }, { embedding: [1, -1.5e-7] }] }
  },
  {
    name: "extra fields",
    response: {
      object: "list",
      data: [{ index: 0, embedding: [3], object: "embedding" }],
      usage: {}
    }
  },
  { name: "not an object", response: "nope" },
  { name: "null", response: null },
  { name: "no data", response: { result: [] } },
  { name: "data not a list", response: { data: { embedding: [1] } } },
  { name: "an entry without embedding", response: { data: [{ embedding: [1] }, { vector: [2] }] } },
  { name: "a non-number in an embedding", response: { data: [{ embedding: [1, "2"] }] } },
  { name: "a null entry", response: { data: [null] } },
  { name: "empty data", response: { data: [] } }
];

const items = [
  { id: "a", content: "alpha", embedding: [1, 0, 0] },
  { id: "b", content: "beta", embedding: [0.9, 0.1, 0], metadata: { source: "doc-1" } },
  { id: "c", content: "gamma", embedding: [0, 1, 0] },
  { id: "d", content: "delta", embedding: [1, 0] },
  { id: "e", content: "zero", embedding: [0, 0, 0] },
  { id: "a", content: "alpha again", embedding: [0.5, 0.5, 0] }
];

const queryCases: QueryCase[] = [
  { name: "top 3", items, embedding: [1, 0, 0], k: 3 },
  { name: "ties keep insertion order", items, embedding: [1, 0], k: 6 },
  { name: "k zero", items, embedding: [1, 0, 0], k: 0 },
  { name: "negative k", items, embedding: [1, 0, 0], k: -2 },
  { name: "k beyond size", items, embedding: [0, 1, 0], k: 50 },
  { name: "a zero query", items, embedding: [0, 0, 0], k: 2 },
  { name: "an empty store", items: [], embedding: [1], k: 3 },
  { name: "a longer query", items, embedding: [1, 0, 0, 1], k: 2 }
];

const cosineCases: CosineCase[] = [
  { name: "same", a: [1, 2, 3], b: [1, 2, 3] },
  { name: "orthogonal", a: [1, 0], b: [0, 1] },
  { name: "opposite", a: [1, 1], b: [-1, -1] },
  { name: "different lengths", a: [1, 2], b: [1, 2, 3] },
  { name: "a zero vector", a: [0, 0], b: [1, 1] },
  { name: "empty", a: [], b: [] },
  { name: "fractions", a: [0.1, 0.2, 0.3], b: [0.3, 0.2, 0.1] }
];

/** What `createEmbeddings` sends for a body case, through an injected transport. */
const sentBody = async ({ options, texts }: BodyCase): Promise<unknown> => {
  let sent: unknown = null;
  const embeddings = createEmbeddings({
    ...(options as object),
    transport: (body) => {
      sent = body;
      return { data: texts.map(() => ({ embedding: [0] })) };
    }
  });
  await embeddings.embed(texts);
  return sent;
};

/** How `createEmbeddings` reads a response: the vectors, or the error message. */
const readResponse = async ({ response }: ResponseCase): Promise<unknown> => {
  const embeddings = createEmbeddings({ transport: () => response });
  try {
    return { vectors: await embeddings.embed(["t"]) };
  } catch (error) {
    return { error: (error as Error).message };
  }
};

const queried = ({ items: stored, embedding, k }: QueryCase): unknown => {
  const store = createVectorStore();
  store.upsert(stored as never);
  return { size: store.size(), matches: store.query(embedding, k) };
};

const record = async () => ({
  bodies: await Promise.all(bodyCases.map(async (c) => ({ ...c, expected: await sentBody(c) }))),
  responses: await Promise.all(
    responseCases.map(async (c) => ({ ...c, expected: await readResponse(c) }))
  ),
  queries: queryCases.map((c) => ({ ...c, expected: queried(c) })),
  cosines: cosineCases.map((c) => ({ ...c, expected: cosineSimilarity(c.a, c.b) }))
});

const napi = (): {
  engineEmbeddingsBody: (optionsJson: string, textsJson: string) => string;
  engineParseEmbeddingsResponse: (responseJson: string) => string;
  engineQueryVectors: (itemsJson: string, embeddingJson: string, k: number) => string;
  engineCosineSimilarity: (aJson: string, bJson: string) => number;
} => createRequire(import.meta.url)("@ailu-ai/napi");

/** What the engine gives for the same cases, through `@ailu-ai/napi`. */
const engineRecord = () => {
  const native = napi();
  return {
    bodies: bodyCases.map((c) => ({
      ...c,
      expected:
        c.texts.length === 0
          ? null
          : JSON.parse(
              native.engineEmbeddingsBody(JSON.stringify(c.options), JSON.stringify(c.texts))
            )
    })),
    responses: responseCases.map((c) => {
      try {
        return {
          ...c,
          expected: {
            vectors: JSON.parse(native.engineParseEmbeddingsResponse(JSON.stringify(c.response)))
          }
        };
      } catch (error) {
        // The TypeScript message starts with its function's name.
        return { ...c, expected: { error: `createEmbeddings: ${(error as Error).message}` } };
      }
    }),
    queries: queryCases.map((c) => {
      const store = new Map<string, unknown>();
      for (const item of c.items as { id: string }[]) store.set(item.id, item);
      return {
        ...c,
        expected: {
          size: store.size,
          matches: JSON.parse(
            native.engineQueryVectors(
              JSON.stringify([...store.values()]),
              JSON.stringify(c.embedding),
              c.k
            )
          )
        }
      };
    }),
    cosines: cosineCases.map((c) => ({
      ...c,
      expected: native.engineCosineSimilarity(JSON.stringify(c.a), JSON.stringify(c.b))
    }))
  };
};

describe("@ailu-ai/graph-sdk — embeddings and vector store golden cases (ADR 0045 M4)", () => {
  if (process.env.AILU_UPDATE_EMBEDDINGS_GOLDEN === "1") {
    it("rewrites the golden file from this SDK", async () => {
      writeFileSync(FIXTURE, `${JSON.stringify(await record(), null, 2)}\n`);
    });
    return;
  }

  it("embeds, reads responses, queries and scores as recorded", async () => {
    expect(JSON.parse(JSON.stringify(await record()))).toEqual(
      JSON.parse(readFileSync(FIXTURE, "utf8"))
    );
  });

  (rustEngineAvailable() ? it : it.skip)("the engine gives the recorded results too", () => {
    expect(JSON.parse(JSON.stringify(engineRecord()))).toEqual(
      JSON.parse(readFileSync(FIXTURE, "utf8"))
    );
  });
});
