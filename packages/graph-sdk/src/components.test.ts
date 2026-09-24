import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { components } from "./components.js";
import type { HttpFetchRequestInit, HttpFetchResponseLike } from "./components.js";
import { createGraph, rustEngineAvailable } from "./index.js";

/**
 * Component-node tests. The TS-fallback handler tests run everywhere; the
 * "through Rust" test runs only when the native addon is present (built via
 * `scripts/build-napi.sh`) and is forced onto the Rust engine so the native
 * component handler is exercised.
 */
describe("@ailu/graph-sdk — components (TS fallback handlers)", () => {
  it("promptBuilder renders {{var}} placeholders from the channels", async () => {
    const app = createGraph({ name: "prompt-ts" })
      .channel("name", { type: "string", default: "" })
      .channel("role", { type: "string", default: "" })
      .channel("prompt", { type: "string", default: "" })
      .component("build", components.promptBuilder({ template: "Hello {{ name }}, you are {{role}}.", into: "prompt" }))
      .compile();

    const result = await app.run({ name: "Ada", role: "admin" });
    expect((result.channels as Record<string, string>).prompt).toBe("Hello Ada, you are admin.");
  });

  it("promptBuilder assembles a RetrievalCapsule when configured (ADR 0044 D3)", async () => {
    const app = createGraph({ name: "prompt-capsule-ts" })
      .channel("question", { type: "string", default: "" })
      .channel("reranked", { type: "json", default: [] as unknown[] })
      .channel("fusedDiscarded", { type: "json", default: [] as unknown[] })
      .channel("prompt", { type: "string", default: "" })
      .channel("capsule", { type: "json", default: null })
      .component(
        "build",
        components.promptBuilder({
          template: "Facts:\n{{reranked}}\n\nQuestion: {{question}}",
          into: "prompt",
          capsule: { into: "capsule", chunksFrom: "reranked", queryFrom: "question", discardedFrom: ["fusedDiscarded"] }
        })
      )
      .compile();

    const result = await app.run({
      question: "what is cat",
      reranked: [
        {
          id: "d1",
          content: "cat facts",
          score: 0.9,
          provenance: [{ stage: "rerank", algorithm: "cross-encoder", algorithmVersion: null, score: 0.9 }]
        }
      ],
      fusedDiscarded: [
        {
          id: "d2",
          content: "unrelated",
          score: 0.1,
          provenance: [{ stage: "rrf", algorithm: "rrf", algorithmVersion: "k=60", score: 0.1 }],
          discardedAt: "1700000000000",
          reason: "rrf_rank_below_k"
        }
      ]
    });

    const channels = result.channels as Record<string, unknown>;
    const prompt = channels.prompt as string;
    expect(prompt.startsWith("Facts:\n")).toBe(true);
    expect(prompt.includes("cat facts")).toBe(true);
    expect(prompt.endsWith("Question: what is cat")).toBe(true);

    const capsule = channels.capsule as {
      order: string[];
      chunks: Array<{ id: string; provenance: unknown[] }>;
      discarded: Array<{ id: string; reason: string }>;
      renderedHash: string;
      queryHash: string;
    };
    expect(capsule.order).toEqual(["d1"]);
    expect(capsule.chunks).toHaveLength(1);
    expect(capsule.chunks[0]!.id).toBe("d1");
    expect(capsule.chunks[0]!.provenance).toHaveLength(1);
    expect(capsule.discarded).toHaveLength(1);
    expect(capsule.discarded[0]!.id).toBe("d2");
    expect(capsule.discarded[0]!.reason).toBe("rrf_rank_below_k");
    expect(capsule.renderedHash).toHaveLength(64);
    expect(capsule.queryHash).toHaveLength(64);
    expect(capsule.renderedHash).not.toBe(capsule.queryHash);
  });

  it("jsonValidator reports missing keys and type mismatch", async () => {
    const app = createGraph({ name: "validate-ts" })
      .channel("payload", { type: "json" })
      .channel("ok", { type: "boolean", default: false })
      .channel("errs", { type: "json", default: [] })
      .component(
        "validate",
        components.jsonValidator({
          from: "payload",
          requiredKeys: ["a", "b"],
          expectType: "object",
          okInto: "ok",
          errorsInto: "errs"
        })
      )
      .compile();

    const result = await app.run({ payload: { a: 1 } });
    const channels = result.channels as Record<string, unknown>;
    expect(channels.ok).toBe(false);
    expect(channels.errs).toEqual(["missing required key `b`"]);
  });

  it("outputParser extracts the first JSON object from prose (skipping string braces)", async () => {
    const app = createGraph({ name: "parse-ts" })
      .channel("text", { type: "string", default: "" })
      .channel("parsed", { type: "json", default: null })
      .component("parse", components.outputParser({ from: "text", into: "parsed" }))
      .compile();

    const result = await app.run({ text: 'noise [ {"k": "}"}, 2 ] tail' });
    expect((result.channels as Record<string, unknown>).parsed).toEqual([{ k: "}" }, 2]);
  });

  it("router picks the first matching rule then falls back to the default", async () => {
    const build = () =>
      createGraph({ name: "route-ts" })
        .channel("label", { type: "string", default: "" })
        .channel("route", { type: "string", default: "" })
        .component(
          "route",
          components.router({
            from: "label",
            rules: [
              { equals: "spam", route: "drop" },
              { contains: "urgent", route: "escalate" }
            ],
            defaultRoute: "inbox",
            into: "route"
          })
        )
        .compile();

    expect(((await build().run({ label: "spam" })).channels as Record<string, string>).route).toBe("drop");
    expect(((await build().run({ label: "this is urgent!" })).channels as Record<string, string>).route).toBe(
      "escalate"
    );
    expect(((await build().run({ label: "hello" })).channels as Record<string, string>).route).toBe("inbox");
  });

  it("retriever returns descending-scored top-k results", async () => {
    const app = createGraph({ name: "retrieve-ts" })
      .channel("q", { type: "string", default: "" })
      .channel("hits", { type: "json", default: [] })
      .component(
        "retrieve",
        components.retriever({
          query: "q",
          into: "hits",
          k: 2,
          docs: [
            { id: "d1", content: "critical risk alert" },
            { id: "d2", content: "general weather update" },
            { id: "d3", content: "critical risk warning" }
          ]
        })
      )
      .compile();

    const result = await app.run({ q: "critical risk" });
    const channels = result.channels as unknown as Record<string, unknown>;
    const hits = channels.hits as { id: string; score: number }[];
    expect(hits).toHaveLength(2);
    expect(hits[0]!.score).toBeGreaterThanOrEqual(hits[1]!.score);
    expect(hits[0]!.id).toBeDefined();
  });

  it("reranker reorders by existing score without a query", async () => {
    const app = createGraph({ name: "rerank-ts" })
      .channel("hits", { type: "json", default: [] as unknown[] })
      .channel("ranked", { type: "json", default: [] as unknown[] })
      .component("rerank", components.reranker({ from: "hits", into: "ranked" }))
      .compile();

    const result = await app.run({
      hits: [
        { id: "a", content: "x", score: 0.1 },
        { id: "b", content: "y", score: 0.9 },
        { id: "c", content: "z", score: 0.5 }
      ] as unknown[]
    });
    const channels = result.channels as unknown as Record<string, unknown>;
    const ranked = channels.ranked as { id: string }[];
    expect(ranked.map((r) => r.id)).toEqual(["b", "c", "a"]);
  });

  it("reranker with a query but no cross-encoder keeps the upstream ranking", async () => {
    const saved = process.env.AILU_RERANK_ENDPOINT;
    delete process.env.AILU_RERANK_ENDPOINT;
    try {
      const app = createGraph({ name: "rerank-query-ts" })
        .channel("q", { type: "string", default: "" })
        .channel("hits", { type: "json", default: [] as unknown[] })
        .channel("ranked", { type: "json", default: [] as unknown[] })
        .component("rerank", components.reranker({ from: "hits", into: "ranked", query: "q" }))
        .compile();

      const result = await app.run({
        q: "critical risk",
        hits: [
          { id: "a", content: "general update", score: 0.9 },
          { id: "b", content: "critical risk alert", score: 0.1 }
        ] as unknown[]
      });
      const ranked = (result.channels as unknown as Record<string, unknown>).ranked as { id: string }[];
      expect(ranked.map((r) => r.id)).toEqual(["a", "b"]);
    } finally {
      if (saved !== undefined) process.env.AILU_RERANK_ENDPOINT = saved;
    }
  });

  it("textCleaner applies strip/lowercase/collapse/trim in a fixed order", async () => {
    const app = createGraph({ name: "clean-ts" })
      .channel("raw", { type: "string", default: "" })
      .channel("clean", { type: "string", default: "" })
      .component(
        "clean",
        components.textCleaner({
          from: "raw",
          into: "clean",
          lowercase: true,
          stripHtml: true,
          collapseWhitespace: true,
          trim: true
        })
      )
      .compile();

    const result = await app.run({ raw: "  <b>Hello</b>   WORLD  " });
    expect((result.channels as Record<string, string>).clean).toBe("hello world");
  });

  it("documentSplitter by chars slides windows with overlap", async () => {
    const app = createGraph({ name: "split-ts" })
      .channel("doc", { type: "string", default: "" })
      .channel("chunks", { type: "json", default: [] })
      .component(
        "split",
        components.documentSplitter({ from: "doc", into: "chunks", by: "chars", size: 4, overlap: 1 })
      )
      .compile();

    const result = await app.run({ doc: "abcdefgh" });
    expect((result.channels as Record<string, unknown>).chunks).toEqual(["abcd", "defg", "gh"]);
  });

  it("htmlToText strips tags and decodes entities (amp last)", async () => {
    const app = createGraph({ name: "html-ts" })
      .channel("html", { type: "string", default: "" })
      .channel("text", { type: "string", default: "" })
      .component("html", components.htmlToText({ from: "html", into: "text" }))
      .compile();

    const result = await app.run({ html: "<p>Tom &amp; Jerry say &lt;hi&gt; &quot;there&quot;</p>" });
    expect((result.channels as Record<string, string>).text).toBe('Tom & Jerry say <hi> "there"');
  });

  it("csvParser with header yields row objects", async () => {
    const app = createGraph({ name: "csv-ts" })
      .channel("csv", { type: "string", default: "" })
      .channel("rows", { type: "json", default: [] })
      .component("csv", components.csvParser({ from: "csv", into: "rows" }))
      .compile();

    const result = await app.run({ csv: "name,age\nAda,36\nBob,40" });
    expect((result.channels as Record<string, unknown>).rows).toEqual([
      { name: "Ada", age: "36" },
      { name: "Bob", age: "40" }
    ]);
  });

  it("documentJoiner merges arrays in channel order and dedupes by field", async () => {
    const app = createGraph({ name: "join-ts" })
      .channel("a", { type: "json", default: [] as unknown[] })
      .channel("b", { type: "json", default: [] as unknown[] })
      .channel("merged", { type: "json", default: [] as unknown[] })
      .component(
        "join",
        components.documentJoiner({ fromChannels: ["a", "b"], into: "merged", dedupeBy: "id" })
      )
      .compile();

    const result = await app.run({
      a: [
        { id: "x", v: 1 },
        { id: "y", v: 2 }
      ] as unknown[],
      b: [
        { id: "x", v: 9 },
        { id: "z", v: 3 }
      ] as unknown[]
    });
    const merged = (result.channels as Record<string, unknown>).merged as { id: string; v: number }[];
    expect(merged.map((m) => m.id)).toEqual(["x", "y", "z"]);
    expect(merged[0]!.v).toBe(1);
  });

  it("deduplicator keeps the first occurrence by whole value", async () => {
    const app = createGraph({ name: "dedupe-ts" })
      .channel("items", { type: "json", default: [] as unknown[] })
      .channel("out", { type: "json", default: [] as unknown[] })
      .component("dedupe", components.deduplicator({ from: "items", into: "out" }))
      .compile();

    const result = await app.run({ items: ["a", "b", "a", "c", "b"] as unknown[] });
    expect((result.channels as Record<string, unknown>).out).toEqual(["a", "b", "c"]);
  });

  it("truncator truncates and appends the ellipsis within the budget", async () => {
    const app = createGraph({ name: "trunc-ts" })
      .channel("text", { type: "string", default: "" })
      .channel("out", { type: "string", default: "" })
      .component("trunc", components.truncator({ from: "text", into: "out", maxChars: 10, ellipsis: "..." }))
      .compile();

    const result = await app.run({ text: "abcdefghijklmnop" });
    expect((result.channels as Record<string, string>).out).toBe("abcdefg...");
  });

  it("regexExtractor returns every literal occurrence with all:true", async () => {
    const app = createGraph({ name: "regex-ts" })
      .channel("text", { type: "string", default: "" })
      .channel("out", { type: "json", default: null })
      .component("regex", components.regexExtractor({ from: "text", into: "out", pattern: "ab", all: true }))
      .compile();

    const result = await app.run({ text: "ab cab dab" });
    expect((result.channels as Record<string, unknown>).out).toEqual(["ab", "ab", "ab"]);
  });

  it("answerBuilder appends numbered citations with the default layout", async () => {
    const app = createGraph({ name: "answer-ts" })
      .channel("answer", { type: "string", default: "" })
      .channel("ctx", { type: "json", default: [] as unknown[] })
      .channel("final", { type: "string", default: "" })
      .component(
        "build",
        components.answerBuilder({ from: "answer", contextFrom: "ctx", into: "final" })
      )
      .compile();

    const result = await app.run({
      answer: "The sky is blue.",
      ctx: [
        { id: "d1", content: "Rayleigh scattering." },
        { id: "d2", content: "Sunlight is white." }
      ] as unknown[]
    });
    expect((result.channels as Record<string, unknown>).final).toBe(
      "The sky is blue.\n\nSources:\n[1] d1: Rayleigh scattering.\n[2] d2: Sunlight is white."
    );
  });

  it("fieldMapper remaps nested dotted paths and nulls the unresolved", async () => {
    const app = createGraph({ name: "map-ts" })
      .channel("src", { type: "json", default: null as unknown })
      .channel("out", { type: "json", default: null as unknown })
      .component(
        "map",
        components.fieldMapper({
          from: "src",
          into: "out",
          mapping: { fullName: "user.name", city: "address.city", missing: "a.b" }
        })
      )
      .compile();

    const result = await app.run({ src: { user: { name: "Ada" }, address: { city: "London" } } });
    expect((result.channels as Record<string, unknown>).out).toEqual({
      fullName: "Ada",
      city: "London",
      missing: null
    });
  });

  it("fieldExtractor reduces an AgentResult.reasoning trace to its final answer text", async () => {
    const app = createGraph({ name: "extract-ts" })
      .channel("ragResult", { type: "json", default: null as unknown })
      .channel("finalAnswer", { type: "string", default: "" })
      .component(
        "extract",
        components.fieldExtractor({
          from: "ragResult",
          into: "finalAnswer",
          path: "reasoning",
          finalOnly: true
        })
      )
      .compile();

    const result = await app.run({
      ragResult: {
        reasoning: "thought: ground the answer\nfinal:The sky is blue [d1].",
        requiresHumanReview: false,
        approvalRequests: []
      }
    });
    expect((result.channels as Record<string, unknown>).finalAnswer).toBe("The sky is blue [d1].");
  });

  it("bm25Retriever ranks lexical overlap first and excludes unrelated docs", async () => {
    const app = createGraph({ name: "bm25-ts" })
      .channel("q", { type: "string", default: "" })
      .channel("hits", { type: "json", default: [] })
      .component(
        "bm25",
        components.bm25Retriever({
          query: "q",
          into: "hits",
          k: 2,
          docs: [
            { id: "d1", content: "the cat sat on the mat" },
            { id: "d2", content: "quantum field theory lecture" },
            { id: "d3", content: "a cat and a dog" }
          ]
        })
      )
      .compile();

    const result = await app.run({ q: "cat" });
    const hits = (result.channels as Record<string, unknown>).hits as { id: string; score: number }[];
    expect(hits).toHaveLength(2);
    const ids = hits.map((h) => h.id);
    expect(ids).toContain("d1");
    expect(ids).toContain("d3");
    expect(ids).not.toContain("d2");
    expect(hits[0]!.score).toBeGreaterThanOrEqual(hits[1]!.score);
  });

  it("bm25Retriever attaches its own provenance step instead of a bare score (ADR 0044)", async () => {
    const app = createGraph({ name: "bm25-provenance-ts" })
      .channel("q", { type: "string", default: "" })
      .channel("hits", { type: "json", default: [] })
      .component(
        "bm25",
        components.bm25Retriever({
          query: "q",
          into: "hits",
          k: 1,
          k1: 1.5,
          b: 0.6,
          docs: [{ id: "d1", content: "cat" }]
        })
      )
      .compile();

    const result = await app.run({ q: "cat" });
    const hits = (result.channels as Record<string, unknown>).hits as Array<{
      score: number;
      provenance: Array<{ stage: string; algorithm: string; algorithmVersion: string | null; score: number }>;
    }>;
    expect(hits[0]!.provenance).toHaveLength(1);
    expect(hits[0]!.provenance[0]!.stage).toBe("bm25");
    expect(hits[0]!.provenance[0]!.algorithm).toBe("bm25");
    expect(hits[0]!.provenance[0]!.algorithmVersion).toBe("k1=1.5,b=0.6");
    expect(hits[0]!.provenance[0]!.score).toBe(hits[0]!.score);
  });

  it("bm25Retriever writes the rank>k loser to a discarded channel with a reason (ADR 0044 D2)", async () => {
    const app = createGraph({ name: "bm25-discard-ts" })
      .channel("q", { type: "string", default: "" })
      .channel("hits", { type: "json", default: [] })
      .channel("hitsDiscarded", { type: "json", default: [] })
      .component(
        "bm25",
        components.bm25Retriever({
          query: "q",
          into: "hits",
          k: 1,
          docs: [
            { id: "d1", content: "cat cat cat" },
            { id: "d2", content: "cat" }
          ]
        })
      )
      .compile();

    const result = await app.run({ q: "cat" });
    const channels = result.channels as Record<string, unknown>;
    const hits = channels.hits as Array<{ id: string }>;
    const discarded = channels.hitsDiscarded as Array<{
      id: string;
      reason: string;
      discardedAt: string;
      provenance: Array<{ stage: string }>;
    }>;
    expect(hits).toHaveLength(1);
    expect(hits[0]!.id).toBe("d1");
    expect(discarded).toHaveLength(1);
    expect(discarded[0]!.id).toBe("d2");
    expect(discarded[0]!.reason).toBe("bm25_rank_below_k");
    expect(typeof discarded[0]!.discardedAt).toBe("string");
    expect(discarded[0]!.provenance[0]!.stage).toBe("bm25");
  });

  it("keywordRetriever scores by query-term coverage", async () => {
    const app = createGraph({ name: "kw-ts" })
      .channel("q", { type: "string", default: "" })
      .channel("hits", { type: "json", default: [] })
      .component(
        "kw",
        components.keywordRetriever({
          query: "q",
          into: "hits",
          docs: [
            { id: "d1", content: "red green blue" },
            { id: "d2", content: "red yellow" }
          ]
        })
      )
      .compile();

    const result = await app.run({ q: "red green" });
    const hits = (result.channels as Record<string, unknown>).hits as { id: string; score: number }[];
    expect(hits[0]!.id).toBe("d1");
    expect(hits[0]!.score).toBe(1);
    expect(hits[1]!.score).toBe(0.5);
  });

  it("sentenceWindowSplitter slides overlapping sentence windows by stride", async () => {
    const app = createGraph({ name: "sw-ts" })
      .channel("doc", { type: "string", default: "" })
      .channel("win", { type: "json", default: [] })
      .component(
        "sw",
        components.sentenceWindowSplitter({ from: "doc", into: "win", windowSize: 2, stride: 1 })
      )
      .compile();

    const result = await app.run({ doc: "One. Two. Three. Four." });
    expect((result.channels as Record<string, unknown>).win).toEqual([
      "One. Two.",
      "Two. Three.",
      "Three. Four."
    ]);
  });

  it("languageDetector picks the dominant language and reports confidence", async () => {
    const app = createGraph({ name: "lang-ts" })
      .channel("txt", { type: "string", default: "" })
      .channel("lang", { type: "string", default: "" })
      .channel("conf", { type: "number", default: 0 })
      .component(
        "lang",
        components.languageDetector({ from: "txt", into: "lang", confidenceInto: "conf" })
      )
      .compile();

    const result = await app.run({ txt: "le chat est dans la maison et il fait chaud" });
    const channels = result.channels as Record<string, unknown>;
    expect(channels.lang).toBe("fr");
    expect(channels.conf as number).toBeGreaterThan(0);
  });

  it("metadataFilter keeps items satisfying a numeric predicate", async () => {
    const app = createGraph({ name: "meta-ts" })
      .channel("docs", { type: "json", default: [] as unknown[] })
      .channel("out", { type: "json", default: [] as unknown[] })
      .component(
        "filter",
        components.metadataFilter({ from: "docs", into: "out", field: "score", op: "gte", value: 0.5 })
      )
      .compile();

    const result = await app.run({
      docs: [
        { id: "a", score: 0.9 },
        { id: "b", score: 0.2 },
        { id: "c", score: 0.5 }
      ] as unknown[]
    });
    const out = (result.channels as Record<string, unknown>).out as { id: string }[];
    expect(out.map((d) => d.id)).toEqual(["a", "c"]);
  });

  it("listJoiner unions array channels (dedupe) and interleaves them", async () => {
    const build = (mode: "union" | "interleave") =>
      createGraph({ name: `list-${mode}-ts` })
        .channel("a", { type: "json", default: [] as unknown[] })
        .channel("b", { type: "json", default: [] as unknown[] })
        .channel("out", { type: "json", default: [] as unknown[] })
        .component("join", components.listJoiner({ fromChannels: ["a", "b"], into: "out", mode }))
        .compile();

    const union = await build("union").run({ a: [1, 2] as unknown[], b: [2, 3] as unknown[] });
    expect((union.channels as Record<string, unknown>).out).toEqual([1, 2, 3]);

    const interleave = await build("interleave").run({ a: [1, 2] as unknown[], b: [2, 3] as unknown[] });
    expect((interleave.channels as Record<string, unknown>).out).toEqual([1, 2, 2, 3]);
  });

  it("mergeRanker fuses streams with RRF so a doc in both lists wins", async () => {
    const app = createGraph({ name: "merge-ts" })
      .channel("lex", { type: "json", default: [] as unknown[] })
      .channel("vec", { type: "json", default: [] as unknown[] })
      .channel("fused", { type: "json", default: [] as unknown[] })
      .component("merge", components.mergeRanker({ fromChannels: ["lex", "vec"], into: "fused" }))
      .compile();

    const result = await app.run({
      lex: [
        { id: "a", content: "x" },
        { id: "b", content: "y" }
      ] as unknown[],
      vec: [
        { id: "b", content: "y" },
        { id: "c", content: "z" }
      ] as unknown[]
    });
    const fused = (result.channels as Record<string, unknown>).fused as { id: string; score: number }[];
    expect(fused[0]!.id).toBe("b");
    expect(fused).toHaveLength(3);
    expect(fused[0]!.score).toBeGreaterThan(0);
  });

  it("mergeRanker preserves BOTH legs' provenance for a doc seen in two channels (ADR 0044)", async () => {
    type Step = { stage: string; algorithm: string; algorithmVersion: string | null; score: number };
    const lexStep: Step = { stage: "bm25", algorithm: "bm25", algorithmVersion: null, score: 1.1 };
    const vecStep: Step = { stage: "vector", algorithm: "cosine", algorithmVersion: null, score: 0.9 };

    const app = createGraph({ name: "merge-provenance-ts" })
      .channel("lex", { type: "json", default: [] as unknown[] })
      .channel("vec", { type: "json", default: [] as unknown[] })
      .channel("fused", { type: "json", default: [] as unknown[] })
      .component("merge", components.mergeRanker({ fromChannels: ["lex", "vec"], into: "fused" }))
      .compile();

    const result = await app.run({
      lex: [{ id: "b", content: "y", score: 1.1, provenance: [lexStep] }] as unknown[],
      vec: [
        { id: "b", content: "y", score: 0.9, provenance: [vecStep] },
        { id: "c", content: "z", score: 0.5, provenance: [] }
      ] as unknown[]
    });
    const fused = (result.channels as Record<string, unknown>).fused as Array<{
      id: string;
      provenance: Step[];
    }>;
    const b = fused.find((r) => r.id === "b")!;
    // Both legs' steps survive, plus mergeRanker's own rrf step — none overwritten.
    expect(b.provenance).toHaveLength(3);
    expect(b.provenance.map((s) => s.stage)).toEqual(["bm25", "vector", "rrf"]);

    const c = fused.find((r) => r.id === "c")!;
    expect(c.provenance.map((s) => s.stage)).toEqual(["rrf"]);
  });

  it("mergeRanker writes RRF losers to a discarded channel with a reason (ADR 0044 D2)", async () => {
    const app = createGraph({ name: "merge-discard-ts" })
      .channel("lex", { type: "json", default: [] as unknown[] })
      .channel("vec", { type: "json", default: [] as unknown[] })
      .channel("fused", { type: "json", default: [] as unknown[] })
      .channel("fusedDiscarded", { type: "json", default: [] as unknown[] })
      .component("merge", components.mergeRanker({ fromChannels: ["lex", "vec"], into: "fused", k: 1 }))
      .compile();

    const result = await app.run({
      lex: [{ id: "b", content: "y", score: 1.1, provenance: [] }] as unknown[],
      vec: [
        { id: "b", content: "y", score: 0.9, provenance: [] },
        { id: "c", content: "z", score: 0.5, provenance: [] }
      ] as unknown[]
    });
    const channels = result.channels as Record<string, unknown>;
    const fused = channels.fused as Array<{ id: string }>;
    const discarded = channels.fusedDiscarded as Array<{
      id: string;
      reason: string;
      discardedAt: string;
      provenance: Array<{ stage: string }>;
    }>;
    expect(fused).toHaveLength(1);
    expect(fused[0]!.id).toBe("b");
    expect(discarded).toHaveLength(1);
    expect(discarded[0]!.id).toBe("c");
    expect(discarded[0]!.reason).toBe("rrf_rank_below_k");
    expect(typeof discarded[0]!.discardedAt).toBe("string");
    expect(discarded[0]!.provenance.at(-1)!.stage).toBe("rrf");
  });

  it("evaluator scores token-F1 and sets the pass flag against a threshold", async () => {
    const build = () =>
      createGraph({ name: "eval-ts" })
        .channel("exp", { type: "string", default: "" })
        .channel("act", { type: "string", default: "" })
        .channel("score", { type: "number", default: 0 })
        .channel("passed", { type: "boolean", default: false })
        .component(
          "eval",
          components.evaluator({
            expectedFrom: "exp",
            actualFrom: "act",
            into: "score",
            passInto: "passed",
            threshold: 0.5
          })
        )
        .compile();

    const perfect = await build().run({ exp: "the quick brown fox", act: "the quick brown fox" });
    expect((perfect.channels as Record<string, unknown>).score).toBe(1);
    expect((perfect.channels as Record<string, unknown>).passed).toBe(true);

    const miss = await build().run({ exp: "alpha beta", act: "gamma delta" });
    expect((miss.channels as Record<string, unknown>).score).toBe(0);
    expect((miss.channels as Record<string, unknown>).passed).toBe(false);
  });

  it("chatMessageBuilder assembles role-tagged messages with templated content", async () => {
    const app = createGraph({ name: "chat-ts" })
      .channel("sys", { type: "string", default: "" })
      .channel("name", { type: "string", default: "" })
      .channel("reply", { type: "string", default: "" })
      .channel("messages", { type: "json", default: [] as unknown[] })
      .component(
        "chat",
        components.chatMessageBuilder({
          into: "messages",
          systemFrom: "sys",
          messages: [
            { role: "user", content: "Hello {{name}}" },
            { role: "assistant", contentFrom: "reply" }
          ]
        })
      )
      .compile();

    const result = await app.run({ sys: "You are helpful.", name: "Ada", reply: "Hi Ada!" });
    expect((result.channels as Record<string, unknown>).messages).toEqual([
      { role: "system", content: "You are helpful." },
      { role: "user", content: "Hello Ada" },
      { role: "assistant", content: "Hi Ada!" }
    ]);
  });

  it("conditionalRouter picks the first matching branch then the default", async () => {
    const build = () =>
      createGraph({ name: "cond-ts" })
        .channel("score", { type: "number", default: 0 })
        .channel("lang", { type: "string", default: "" })
        .channel("route", { type: "string", default: "" })
        .component(
          "route",
          components.conditionalRouter({
            into: "route",
            defaultRoute: "fallback",
            branches: [
              { when: { field: "score", op: "gte", value: 0.8 }, route: "high" },
              { when: { field: "lang", op: "equals", value: "fr" }, route: "french" }
            ]
          })
        )
        .compile();

    const routeOf = (channels: unknown): unknown =>
      (channels as Record<string, unknown>).route;
    expect(routeOf((await build().run({ score: 0.9 })).channels)).toBe("high");
    expect(routeOf((await build().run({ score: 0.1, lang: "fr" })).channels)).toBe("french");
    expect(routeOf((await build().run({ score: 0.1 })).channels)).toBe("fallback");
  });

  it("documentWriter appends to an existing store and dedupes by field keeping the first", async () => {
    const app = createGraph({ name: "writer-ts" })
      .channel("incoming", { type: "json", default: [] as unknown[] })
      .channel("store", { type: "json", default: [] as unknown[] })
      .component(
        "write",
        components.documentWriter({ from: "incoming", into: "store", dedupeBy: "id" })
      )
      .compile();

    const result = await app.run({
      store: [{ id: "a", v: 1 }] as unknown[],
      incoming: [
        { id: "a", v: 9 },
        { id: "b", v: 2 }
      ] as unknown[]
    });
    const store = (result.channels as Record<string, unknown>).store as { id: string; v: number }[];
    expect(store).toHaveLength(2);
    expect(store[0]!.v).toBe(1);
    expect(store[1]!.id).toBe("b");
  });
});

/**
 * A `Response`-like fake the httpFetch impl can consume: enough of the WHATWG
 * `Response` surface (`status`, `ok`, `text()`, `headers.get()`) for the default
 * impl, with no network. `ok` is derived from the status like the real `Response`.
 */
const fakeResponse = (init: {
  status: number;
  body: string;
  contentType?: string;
}): HttpFetchResponseLike => ({
  status: init.status,
  ok: init.status >= 200 && init.status < 300,
  text: () => init.body,
  headers: {
    get: (name: string) =>
      name.toLowerCase() === "content-type" ? (init.contentType ?? null) : null
  }
});

/**
 * Integration components (the vendor-I/O pattern). `httpFetch`/`webSearch` are NOT
 * Rust components: they are plain JS node handlers (added via `.node(...)`) that take
 * an injectable transport, defaulting to the REAL global fetch / Tavily connector.
 * Every test here injects a fake (or unsets the env key) so it runs end-to-end with
 * NO real network. Forced onto Rust when the addon is present so the JS-seam path
 * (on_node) is exercised; otherwise the TS path.
 */
describe("@ailu/graph-sdk — integration components (injected, offline)", () => {
  const saved: Record<string, string | undefined> = {};
  // When the addon is present, run on Rust so the JS handler crosses the on_node seam;
  // otherwise stay on the TS engine. Either way the injected fake keeps it offline.
  const engine = rustEngineAvailable() ? "rust" : "ts";

  beforeEach(() => {
    saved.AILU_SDK_ENGINE = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = engine;
  });

  afterEach(() => {
    if (saved.AILU_SDK_ENGINE === undefined) {
      delete process.env.AILU_SDK_ENGINE;
    } else {
      process.env.AILU_SDK_ENGINE = saved.AILU_SDK_ENGINE;
    }
  });

  it("httpFetch passes method/headers/body/timeout to the injected fetch and parses JSON", async () => {
    const calls: { url: string; init: HttpFetchRequestInit }[] = [];
    const app = createGraph({ name: "fetch-injected" })
      .channel("url", { type: "string", default: "" })
      .channel("body", { type: "json", default: null })
      .node(
        "post",
        components.httpFetch({
          urlFrom: "url",
          into: "body",
          method: "POST",
          headers: { authorization: "Bearer t0ken", "content-type": "application/json" },
          body: '{"hello":"world"}',
          timeoutMs: 5000,
          fetchImpl: (url, init) => {
            calls.push({ url, init });
            return fakeResponse({
              status: 200,
              body: '{"ok":true,"items":[1,2,3]}',
              contentType: "application/json; charset=utf-8"
            });
          }
        })
      )
      .compile();

    const result = await app.run({ url: "https://example.test/a" }, { runId: "run_http_inject" as never });
    expect(result.status).toBe("completed");

    // Transport passthrough: URL + method + headers + body + an abort signal for the timeout.
    expect(calls).toHaveLength(1);
    expect(calls[0]!.url).toBe("https://example.test/a");
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.headers).toEqual({
      authorization: "Bearer t0ken",
      "content-type": "application/json"
    });
    expect(calls[0]!.init.body).toBe('{"hello":"world"}');
    expect(calls[0]!.init.signal).toBeInstanceOf(AbortSignal);

    const body = (result.channels as Record<string, unknown>).body as {
      status: number;
      ok: boolean;
      body: string;
      json: unknown;
    };
    expect(body.status).toBe(200);
    expect(body.ok).toBe(true);
    expect(body.body).toBe('{"ok":true,"items":[1,2,3]}');
    expect(body.json).toEqual({ ok: true, items: [1, 2, 3] });
  });

  it("httpFetch surfaces a non-2xx status without throwing (ok:false, no json for non-JSON)", async () => {
    const app = createGraph({ name: "fetch-404" })
      .channel("body", { type: "json", default: null })
      .node(
        "get",
        components.httpFetch({
          url: "https://example.test/missing",
          into: "body",
          fetchImpl: () => fakeResponse({ status: 404, body: "not found", contentType: "text/plain" })
        })
      )
      .compile();

    const result = await app.run({}, { runId: "run_http_404" as never });
    expect(result.status).toBe("completed");
    const body = (result.channels as Record<string, unknown>).body as {
      status: number;
      ok: boolean;
      body: string;
      json?: unknown;
    };
    expect(body.status).toBe(404);
    expect(body.ok).toBe(false);
    expect(body.body).toBe("not found");
    expect(body.json).toBeUndefined();
  });

  it("httpFetch surfaces a transport error as { ok:false, error } rather than crashing the run", async () => {
    const app = createGraph({ name: "fetch-error" })
      .channel("body", { type: "json", default: null })
      .node(
        "get",
        components.httpFetch({
          url: "https://example.test/boom",
          into: "body",
          fetchImpl: () => {
            throw new Error("connection refused");
          }
        })
      )
      .compile();

    const result = await app.run({}, { runId: "run_http_err" as never });
    expect(result.status).toBe("completed");
    const body = (result.channels as Record<string, unknown>).body as { ok: boolean; error: string };
    expect(body.ok).toBe(false);
    expect(body.error).toBe("connection refused");
  });

  it("httpFetch defaults to globalThis.fetch (proven by a fake global fetch, then restored)", async () => {
    const original = globalThis.fetch;
    const calls: string[] = [];
    // Replace the global fetch with an offline fake so the DEFAULT path makes no network call.
    globalThis.fetch = ((url: string) => {
      calls.push(String(url));
      return Promise.resolve(
        fakeResponse({ status: 200, body: '{"via":"global"}', contentType: "application/json" })
      );
    }) as unknown as typeof globalThis.fetch;

    try {
      const app = createGraph({ name: "fetch-global-default" })
        .channel("body", { type: "json", default: null })
        // No fetchImpl injected -> the default impl must reach for globalThis.fetch.
        .node("get", components.httpFetch({ url: "https://example.test/global", into: "body" }))
        .compile();

      const result = await app.run({}, { runId: "run_http_global" as never });
      expect(result.status).toBe("completed");
      expect(calls).toEqual(["https://example.test/global"]);
      const body = (result.channels as Record<string, unknown>).body as {
        status: number;
        ok: boolean;
        json: unknown;
      };
      expect(body.status).toBe(200);
      expect(body.ok).toBe(true);
      expect(body.json).toEqual({ via: "global" });
    } finally {
      globalThis.fetch = original;
    }
  });

  it("webSearch runs through the engine with an injected fake search impl (normalized results)", async () => {
    const seen: { query: string; k: number }[] = [];
    const app = createGraph({ name: "search-injected" })
      .channel("q", { type: "string", default: "" })
      .channel("results", { type: "json", default: [] })
      .node(
        "search",
        components.webSearch({
          queryFrom: "q",
          into: "results",
          k: 2,
          searchImpl: (query, k) => {
            seen.push({ query, k });
            return Array.from({ length: k }, (_unused, i) => ({
              title: `fake ${i}`,
              url: `https://fake.test/${i}`,
              snippet: `snip ${i} for ${query}`
            }));
          }
        })
      )
      .compile();

    const result = await app.run({ q: "ailu runtime" }, { runId: "run_search_inject" as never });
    expect(result.status).toBe("completed");
    expect(seen).toEqual([{ query: "ailu runtime", k: 2 }]);
    const outcome = (result.channels as Record<string, unknown>).results as {
      results: { title: string; url: string; snippet: string }[];
    };
    expect(outcome.results).toHaveLength(2);
    expect(outcome.results[0]!.title).toBe("fake 0");
    expect(outcome.results[0]!.url).toBe("https://fake.test/0");
    expect(outcome.results[1]!.snippet).toBe("snip 1 for ailu runtime");
  });

  it("webSearch default Tavily connector POSTs via an injected transport and normalizes results", async () => {
    const savedKey = process.env.TAVILY_API_KEY;
    process.env.TAVILY_API_KEY = "tvly-test-key";
    const calls: { url: string; init: { method: string; headers: Record<string, string>; body: string } }[] =
      [];
    try {
      const app = createGraph({ name: "search-tavily" })
        .channel("q", { type: "string", default: "" })
        .channel("results", { type: "json", default: [] })
        .node(
          "search",
          components.webSearch({
            queryFrom: "q",
            into: "results",
            k: 2,
            // No searchImpl: exercise the REAL Tavily connector, but inject the transport offline.
            transport: (url, init) => {
              calls.push({ url, init });
              return fakeResponse({
                status: 200,
                body: JSON.stringify({
                  results: [
                    { title: "First", url: "https://a.test", content: "alpha snippet" },
                    { title: "Second", url: "https://b.test", content: "beta snippet" }
                  ]
                }),
                contentType: "application/json"
              });
            }
          })
        )
        .compile();

      const result = await app.run({ q: "graph runtime" }, { runId: "run_search_tavily" as never });
      expect(result.status).toBe("completed");

      // Connector contract: POST to Tavily with { api_key, query, max_results }.
      expect(calls).toHaveLength(1);
      expect(calls[0]!.url).toBe("https://api.tavily.com/search");
      expect(calls[0]!.init.method).toBe("POST");
      expect(JSON.parse(calls[0]!.init.body) as unknown).toEqual({
        api_key: "tvly-test-key",
        query: "graph runtime",
        max_results: 2
      });

      const outcome = (result.channels as Record<string, unknown>).results as {
        results: { title: string; url: string; snippet: string }[];
        note?: string;
      };
      expect(outcome.note).toBeUndefined();
      expect(outcome.results).toEqual([
        { title: "First", url: "https://a.test", snippet: "alpha snippet" },
        { title: "Second", url: "https://b.test", snippet: "beta snippet" }
      ]);
    } finally {
      if (savedKey === undefined) {
        delete process.env.TAVILY_API_KEY;
      } else {
        process.env.TAVILY_API_KEY = savedKey;
      }
    }
  });

  it("webSearch degrades gracefully with NO network when TAVILY_API_KEY is unset", async () => {
    const savedKey = process.env.TAVILY_API_KEY;
    delete process.env.TAVILY_API_KEY;
    let transportCalled = false;
    try {
      const app = createGraph({ name: "search-nokey" })
        .channel("q", { type: "string", default: "" })
        .channel("results", { type: "json", default: [] })
        .node(
          "search",
          components.webSearch({
            queryFrom: "q",
            into: "results",
            // A transport that would fail the test if ever called — proves NO network on the no-key path.
            transport: () => {
              transportCalled = true;
              throw new Error("network must not be touched when TAVILY_API_KEY is unset");
            }
          })
        )
        .compile();

      const result = await app.run({ q: "anything" }, { runId: "run_search_nokey" as never });
      expect(result.status).toBe("completed");
      expect(transportCalled).toBe(false);
      const outcome = (result.channels as Record<string, unknown>).results as {
        results: unknown[];
        note: string;
      };
      expect(outcome.results).toEqual([]);
      expect(outcome.note).toContain("TAVILY_API_KEY");
    } finally {
      if (savedKey === undefined) {
        delete process.env.TAVILY_API_KEY;
      } else {
        process.env.TAVILY_API_KEY = savedKey;
      }
    }
  });
});

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu/graph-sdk — components (native Rust handler)", () => {
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    saved.AILU_SDK_ENGINE = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = "rust";
  });

  afterEach(() => {
    if (saved.AILU_SDK_ENGINE === undefined) {
      delete process.env.AILU_SDK_ENGINE;
    } else {
      process.env.AILU_SDK_ENGINE = saved.AILU_SDK_ENGINE;
    }
  });

  it("runs a promptBuilder component node THROUGH RUST and sets its channel", async () => {
    const app = createGraph({ name: "prompt-rust" })
      .channel("name", { type: "string", default: "" })
      .channel("prompt", { type: "string", default: "" })
      .component("build", components.promptBuilder({ template: "Hello {{name}}!", into: "prompt" }))
      .compile();

    // The graph is wired to the Rust engine, and the component runs natively.
    expect(app.usesRustEngine).toBe(true);

    const result = await app.run({ name: "Ada" }, { runId: "run_component_rust" as never });
    expect(result.status).toBe("completed");
    expect((result.channels as Record<string, string>).prompt).toBe("Hello Ada!");
  });
});
