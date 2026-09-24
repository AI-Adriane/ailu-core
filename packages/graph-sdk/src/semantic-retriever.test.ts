import { describe, expect, it } from "vitest";

import type { Embeddings } from "./embeddings.js";
import { createGraph, rustEngineAvailable } from "./index.js";
import { semanticRetriever } from "./semantic-retriever.js";

/**
 * The semantic retriever is a vendor-I/O connector (a plain JS node handler added via
 * `.node(...)`) with an injectable {@link Embeddings}. Every test injects a deterministic
 * fake embedder so it runs end-to-end with NO real network. When the Rust addon is
 * present the graph runs on Rust so the JS handler crosses the on_node seam; otherwise
 * the TS engine. Either way the injected fake keeps it offline.
 */

/**
 * A deterministic fake embedder: maps known phrases to fixed vectors so cosine ranking is
 * predictable. The query about "graph runtime" embeds nearest to the relevant doc.
 */
const fakeEmbeddings = (table: Record<string, number[]>, fallback: number[]): Embeddings => ({
  embed: (texts) => Promise.resolve(texts.map((text) => table[text] ?? fallback))
});

const engine = rustEngineAvailable() ? "rust" : "ts";

describe("@ailu-ai/graph-sdk — semanticRetriever (injected fake embeddings, offline)", () => {
  it("ranks the relevant doc first through a compiled graph (into-channel order)", async () => {
    const savedEngine = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = engine;
    try {
      const docs = [
        { id: "weather", content: "today the weather is sunny" },
        { id: "runtime", content: "ailu is a graph runtime engine" },
        { id: "cooking", content: "a recipe for tomato soup" }
      ];
      // Vectors chosen so the query aligns with the "runtime" doc, then "cooking", then
      // "weather" — a clear, deterministic ranking independent of any real model.
      const embeddings = fakeEmbeddings(
        {
          "ailu is a graph runtime engine": [1, 0, 0],
          "a recipe for tomato soup": [0.5, 0.5, 0],
          "today the weather is sunny": [0, 0, 1],
          "what is the ailu graph runtime": [1, 0.1, 0]
        },
        [0, 0, 0]
      );

      const app = createGraph({ name: "semantic-retriever" })
        .channel("q", { type: "string", default: "" })
        .channel("hits", { type: "json", default: [] })
        .node(
          "retrieve",
          semanticRetriever({
            queryFrom: "q",
            into: "hits",
            k: 3,
            docs,
            embeddings
          })
        )
        .compile();

      const result = await app.run(
        { q: "what is the ailu graph runtime" },
        { runId: "run_semantic_inject" as never }
      );
      expect(result.status).toBe("completed");

      const hits = (result.channels as Record<string, unknown>).hits as {
        id: string;
        content: string;
        score: number;
      }[];
      // The relevant doc ranks first; the full order is deterministic.
      expect(hits.map((h) => h.id)).toEqual(["runtime", "cooking", "weather"]);
      expect(hits[0]!.id).toBe("runtime");
      expect(hits[0]!.content).toBe("ailu is a graph runtime engine");
      expect(hits[0]!.score).toBeGreaterThan(hits[1]!.score);
      expect(hits[1]!.score).toBeGreaterThan(hits[2]!.score);
    } finally {
      if (savedEngine === undefined) {
        delete process.env.AILU_SDK_ENGINE;
      } else {
        process.env.AILU_SDK_ENGINE = savedEngine;
      }
    }
  });

  it("honours k and writes the { id, content, score } projection to the into channel", async () => {
    const savedEngine = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = engine;
    try {
      const embeddings = fakeEmbeddings(
        {
          "doc one": [1, 0],
          "doc two": [0.9, 0.1],
          "doc three": [0, 1],
          query: [1, 0]
        },
        [0, 0]
      );

      const app = createGraph({ name: "semantic-retriever-topk" })
        .channel("hits", { type: "json", default: [] })
        .node(
          "retrieve",
          semanticRetriever({
            query: "query",
            into: "hits",
            k: 2,
            docs: [
              { id: "one", content: "doc one" },
              { id: "two", content: "doc two" },
              { id: "three", content: "doc three" }
            ],
            embeddings
          })
        )
        .compile();

      const result = await app.run({}, { runId: "run_semantic_topk" as never });
      expect(result.status).toBe("completed");
      const hits = (result.channels as Record<string, unknown>).hits as {
        id: string;
        content: string;
        score: number;
      }[];
      expect(hits).toHaveLength(2);
      expect(hits.map((h) => h.id)).toEqual(["one", "two"]);
      // Projection is exactly { id, content, score } — no embedding/metadata leak.
      expect(Object.keys(hits[0]!).sort()).toEqual(["content", "id", "score"]);
    } finally {
      if (savedEngine === undefined) {
        delete process.env.AILU_SDK_ENGINE;
      } else {
        process.env.AILU_SDK_ENGINE = savedEngine;
      }
    }
  });
});
