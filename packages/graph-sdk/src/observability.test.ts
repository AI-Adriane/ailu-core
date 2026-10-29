import { describe, expect, it } from "vitest";

import {
  aggregateCosts,
  buildOtlpPayload,
  computeCost,
  DEFAULT_PRICE_BOOK,
  estimateCost,
  type CostResult,
  type PriceBook,
  type TokenUsage,
  createGraph,
  exportTracesToOtlp,
  type OtlpFetch,
  type RunId
} from "./index.js";
import type { RunEvent } from "@ailu-ai/graph-runtime";
import type { CompiledGraph } from "./compiled-graph.js";

describe("@ailu-ai/graph-sdk — observability (ADR 0028 phase 7)", () => {
  it("computeCost prices usage against the book (unknown model = 0, which estimateCost reports as unknown)", () => {
    // 1M in @ $15 + 1M out @ $75 = $90 for claude-opus-4-8.
    expect(computeCost({ promptTokens: 1e6, completionTokens: 1e6 }, "claude-opus-4-8")).toBeCloseTo(90);
    // Cache-read folds into input at the input rate.
    expect(
      computeCost({ promptTokens: 0, completionTokens: 0, cacheReadTokens: 1e6 }, "claude-opus-4-8")
    ).toBeCloseTo(15);
    // Unknown model → 0 (never guessed).
    expect(computeCost({ promptTokens: 1e6, completionTokens: 1e6 }, "who-knows")).toBe(0);
  });

  it("buildOtlpPayload shapes a valid OTLP/HTTP-JSON traces body", () => {
    const body = JSON.parse(
      buildOtlpPayload(
        "run-1",
        [
          {
            spanId: "aabbccddeeff0011",
            name: "agent",
            startNano: "1000000",
            endNano: "2000000",
            status: 1,
            attributes: { "ailu.cost.usd": 0.5, "gen_ai.usage.input_tokens": 100 }
          }
        ],
        "ailu"
      )
    );
    const span = body.resourceSpans[0].scopeSpans[0].spans[0];
    expect(span.name).toBe("agent");
    expect(span.traceId).toHaveLength(32); // 16 bytes hex
    expect(span.attributes).toContainEqual({ key: "ailu.cost.usd", value: { doubleValue: 0.5 } });
    expect(span.attributes).toContainEqual({
      key: "gen_ai.usage.input_tokens",
      value: { intValue: 100 }
    });
  });

  it("exports a run as an OTLP trace (a root span + a span per node)", async () => {
    const captured: Array<{ url: string; body: unknown }> = [];
    const fetchImpl: OtlpFetch = async (url, init) => {
      captured.push({ url, body: JSON.parse(init.body) });
      return { ok: true, status: 200 };
    };

    const app = createGraph({ name: "obs" })
      .channel("x", { type: "number", default: 0 })
      .channel("y", { type: "number", default: 0 })
      .node("a", async () => ({ x: 1 }))
      .node("b", async () => ({ y: 2 }))
      .edge("a", "b")
      .compile();

    const stop = exportTracesToOtlp(app, { endpoint: "http://otel.local/v1/traces", fetchImpl });
    await app.run({}, { runId: "run_obs" as RunId });
    stop();

    expect(captured.length).toBeGreaterThanOrEqual(1);
    const spans = (captured[0]!.body as { resourceSpans: [{ scopeSpans: [{ spans: { name: string }[] }] }] })
      .resourceSpans[0].scopeSpans[0].spans;
    const names = spans.map((s) => s.name);
    expect(names).toContain("run");
    expect(names).toContain("a");
    expect(names).toContain("b");
  });

  it("is a no-op with no endpoint configured", () => {
    const app = createGraph({ name: "obs2" }).node("a", async () => ({})).compile();
    const stop = exportTracesToOtlp(app, {}); // no endpoint, no env
    expect(typeof stop).toBe("function");
    stop(); // must not throw
  });
  describe("estimateCost", () => {
    const book: PriceBook = {
      m: { inPerMtok: 10, outPerMtok: 30 },
      free: { inPerMtok: 0, outPerMtok: 0 },
      rated: {
        inPerMtok: 10,
        outPerMtok: 30,
        cacheReadPerMtok: 1,
        cacheWritePerMtok: 12,
        promptIncludesCacheReads: false
      },
      rough: { inPerMtok: 10, outPerMtok: 30, indicative: true },
      neg: { inPerMtok: -1, outPerMtok: 30 },
      nan: { inPerMtok: 10, outPerMtok: Number.NaN },
      eur: { inPerMtok: 10, outPerMtok: 30, currency: "EUR" }
    };
    const usage: TokenUsage = { promptTokens: 1e6, completionTokens: 1e6 };

    it("is known, and equal to computeCost, for an explicit non-indicative price", () => {
      const r = estimateCost(usage, "m", book);
      expect(r).toMatchObject({ status: "known", currency: "USD", coverage: 1, unpricedTokens: 0, reasons: [] });
      expect(r.amount).toBeCloseTo(computeCost(usage, "m", book));
    });

    it("is unknown, with a null amount, for a model absent from the book", () => {
      expect(estimateCost({ promptTokens: 7, completionTokens: 3 }, "nope", book)).toMatchObject({
        status: "unknown",
        amount: null,
        coverage: 0,
        unpricedTokens: 10,
        reasons: ["modelNotInBook"]
      });
    });

    it("treats a prototype key as absent", () => {
      expect(estimateCost(usage, "constructor", book).status).toBe("unknown");
    });

    it("treats an explicit zero rate as a valid zero", () => {
      expect(estimateCost(usage, "free", book)).toMatchObject({ status: "known", amount: 0, coverage: 1 });
    });

    it("is estimated with the built-in book", () => {
      const r = estimateCost(usage, "claude-opus-4-8");
      expect(r.status).toBe("estimated");
      expect(r.reasons).toContain("indicativePrice");
      expect(r.amount).toBeCloseTo(90);
      expect(estimateCost(usage, "who-knows").status).toBe("unknown");
    });

    it("is estimated for an indicative entry", () => {
      expect(estimateCost(usage, "rough", book)).toMatchObject({
        status: "estimated",
        reasons: ["indicativePrice"]
      });
    });

    it("estimates cache reads without a rate at the input rate (as computeCost does)", () => {
      const u: TokenUsage = { promptTokens: 0, completionTokens: 0, cacheReadTokens: 1e6 };
      const r = estimateCost(u, "m", book);
      expect(r.status).toBe("estimated");
      expect(r.reasons).toEqual(["cacheApproximated"]);
      expect(r.amount).toBeCloseTo(computeCost(u, "m", book));
    });

    it("is known when cache reads and writes have explicit rates", () => {
      const u: TokenUsage = {
        promptTokens: 1e6,
        completionTokens: 0,
        cacheReadTokens: 1e6,
        cacheWriteTokens: 1e6
      };
      const r = estimateCost(u, "rated", book);
      expect(r.status).toBe("known");
      expect(r.amount).toBeCloseTo(10 + 1 + 12);
    });

    it("counts cache writes without a rate at the input rate, above computeCost", () => {
      const u: TokenUsage = { promptTokens: 0, completionTokens: 0, cacheWriteTokens: 1e6 };
      const r = estimateCost(u, "m", book);
      expect(r.status).toBe("estimated");
      expect(r.amount).toBeCloseTo(10);
      expect(r.amount ?? 0).toBeGreaterThan(computeCost(u, "m", book));
    });

    it("is unknown when the usage or the model is missing", () => {
      expect(estimateCost(undefined, "m", book)).toMatchObject({
        status: "unknown",
        amount: null,
        unpricedTokens: 0,
        reasons: ["missingUsage"]
      });
      expect(estimateCost(usage, undefined, book)).toMatchObject({
        status: "unknown",
        amount: null,
        unpricedTokens: 2e6,
        reasons: ["missingModel"]
      });
    });

    it("is known with an amount of 0 for a zero-token usage", () => {
      expect(estimateCost({ promptTokens: 0, completionTokens: 0 }, "m", book)).toMatchObject({
        status: "known",
        amount: 0,
        coverage: 1,
        unpricedTokens: 0
      });
    });

    it("is unknown for a negative or NaN rate", () => {
      for (const model of ["neg", "nan"]) {
        expect(estimateCost(usage, model, book)).toMatchObject({
          status: "unknown",
          amount: null,
          reasons: ["invalidPrice"]
        });
      }
    });

    it("reports the currency of the price entry", () => {
      expect(estimateCost(usage, "eur", book).currency).toBe("EUR");
    });
  });

  describe("aggregateCosts", () => {
    const result = (over: Partial<CostResult>): CostResult => ({
      status: "known",
      amount: 1,
      currency: "USD",
      coverage: 1,
      unpricedTokens: 0,
      pricedTokens: 100,
      reasons: [],
      ...over
    });
    const unknown = (tokens: number): CostResult =>
      result({ status: "unknown", amount: null, coverage: 0, unpricedTokens: tokens, pricedTokens: 0 });

    it("is known when every result is known", () => {
      const s = aggregateCosts([result({ amount: 1 }), result({ amount: 2 })]);
      expect(s).toMatchObject({ status: "known", coverage: 1, complete: true, calls: { known: 2, estimated: 0, unknown: 0 } });
      expect(s.amount).toBeCloseTo(3);
    });

    it("is estimated when anything measured is estimated", () => {
      const s = aggregateCosts([result({}), result({ status: "estimated", amount: 2 })]);
      expect(s).toMatchObject({ status: "estimated", complete: true });
      expect(s.amount).toBeCloseTo(3);
    });

    it("keeps the unmeasured share of a partial total and sums the measured part only", () => {
      const s = aggregateCosts([result({ status: "estimated", amount: 2, pricedTokens: 300 }), unknown(100)]);
      expect(s).toMatchObject({
        status: "estimated",
        complete: false,
        unpricedTokens: 100,
        calls: { known: 0, estimated: 1, unknown: 1 }
      });
      expect(s.amount).toBeCloseTo(2);
      expect(s.coverage).toBeCloseTo(0.75);
    });

    it("is unknown when nothing was measured", () => {
      expect(aggregateCosts([unknown(50)])).toMatchObject({
        status: "unknown",
        amount: null,
        coverage: 0,
        complete: false,
        unpricedTokens: 50
      });
    });

    it("is unknown, not 0 and not complete, for an empty list", () => {
      expect(aggregateCosts([])).toMatchObject({
        status: "unknown",
        amount: null,
        coverage: 0,
        complete: false,
        unpricedTokens: 0
      });
    });

    it("does not add two currencies", () => {
      const s = aggregateCosts([result({ amount: 1 }), result({ amount: 5, currency: "EUR", pricedTokens: 100 })]);
      expect(s).toMatchObject({ currency: "USD", complete: false, status: "estimated", unpricedTokens: 100 });
      expect(s.amount).toBeCloseTo(1);
      expect(s.coverage).toBeCloseTo(0.5);
    });

    it("depends on the order of addition only in the last digit", () => {
      const parts = [0.1, 0.2, 0.3].map((amount) => result({ amount }));
      const forward = aggregateCosts(parts).amount;
      const backward = aggregateCosts([...parts].reverse()).amount;
      expect(forward).not.toBe(backward);
      expect(backward).toBeCloseTo(forward ?? Number.NaN, 12);
      expect(forward).toBeCloseTo(0.6, 12);
    });
  });

  describe("usage and price entries that cannot be trusted", () => {
    const book: PriceBook = { m: { inPerMtok: 10, outPerMtok: 30 } };

    it.each([
      ["NaN prompt tokens", { promptTokens: Number.NaN, completionTokens: 1 }],
      ["infinite completion tokens", { promptTokens: 1, completionTokens: Number.POSITIVE_INFINITY }],
      ["negative prompt tokens", { promptTokens: -5, completionTokens: 1 }],
      ["negative cache reads", { promptTokens: 1, completionTokens: 1, cacheReadTokens: -1 }],
      ["NaN cache writes", { promptTokens: 1, completionTokens: 1, cacheWriteTokens: Number.NaN }]
    ])("%s: unknown, with no amount", (_name, usage) => {
      const r = estimateCost(usage as TokenUsage, "m", book);
      expect(r.status).toBe("unknown");
      expect(r.amount).toBeNull();
      expect(r.reasons).toEqual(["invalidUsage"]);
      expect(Number.isNaN(r.coverage)).toBe(false);
    });

    it.each([
      ["missing prompt tokens", { completionTokens: 1 }],
      ["missing completion tokens", { promptTokens: 1 }],
      ["an empty usage", {}]
    ])("%s: unknown (invalidUsage), the counts are required", (_name, usage) => {
      const r = estimateCost(usage as unknown as TokenUsage, "m", book);
      expect(r.status).toBe("unknown");
      expect(r.amount).toBeNull();
      expect(r.reasons).toEqual(["invalidUsage"]);
    });

    it("an amount that overflows is unknown, not Infinity", () => {
      const r = estimateCost({ promptTokens: 1e308, completionTokens: 1e308 }, "m", {
        m: { inPerMtok: 1e308, outPerMtok: 1e308 }
      });
      expect(r.status).toBe("unknown");
      expect(r.amount).toBeNull();
      expect(r.reasons).toEqual(["invalidUsage"]);
    });

    it("a null price book entry is an invalid price, not a crash", () => {
      const r = estimateCost({ promptTokens: 1, completionTokens: 1 }, "m", {
        m: null as unknown as PriceBook[string]
      });
      expect(r.status).toBe("unknown");
      expect(r.reasons).toEqual(["invalidPrice"]);
    });

    it("aggregateCosts does not produce NaN from a result built without pricedTokens", () => {
      const handBuilt = {
        status: "known",
        amount: 2,
        currency: "USD",
        coverage: 1,
        unpricedTokens: 0,
        reasons: []
      } as unknown as CostResult;
      const summary = aggregateCosts([handBuilt]);
      expect(Number.isNaN(summary.coverage)).toBe(false);
      expect(summary.amount).toBe(2);
    });
  });

  describe("cache reads counted inside the prompt tokens", () => {
    const book: PriceBook = {
      inclusive: { inPerMtok: 10, outPerMtok: 0, cacheReadPerMtok: 1, promptIncludesCacheReads: true },
      apart: { inPerMtok: 10, outPerMtok: 0, cacheReadPerMtok: 1, promptIncludesCacheReads: false },
      unsaid: { inPerMtok: 10, outPerMtok: 0, cacheReadPerMtok: 1 }
    };
    const cached: TokenUsage = { promptTokens: 1e6, completionTokens: 0, cacheReadTokens: 5e5 };

    it("bills an inclusive cache read once, at the cache-read rate", () => {
      const r = estimateCost(cached, "inclusive", book);
      expect(r.status).toBe("known");
      expect(r.amount).toBeCloseTo(5.5, 9);
      expect(r.pricedTokens).toBe(1e6);
    });

    it("adds an apart cache read to the prompt tokens", () => {
      const r = estimateCost(cached, "apart", book);
      expect(r.status).toBe("known");
      expect(r.amount).toBeCloseTo(10.5, 9);
    });

    it("says estimated when the entry does not tell which one it is", () => {
      const r = estimateCost(cached, "unsaid", book);
      expect(r.status).toBe("estimated");
      expect(r.reasons).toContain("cacheApproximated");
    });

    it("says estimated when an inclusive usage reports more cache reads than prompt tokens", () => {
      const r = estimateCost({ promptTokens: 1e5, completionTokens: 0, cacheReadTokens: 5e5 }, "inclusive", book);
      expect(r.status).toBe("estimated");
      expect(r.reasons).toContain("cacheApproximated");
      expect(r.amount).toBeCloseTo(0.5, 9);
    });

    it("knows which built-in entries count cache reads inside the prompt", () => {
      expect(DEFAULT_PRICE_BOOK["gpt-4o"]?.promptIncludesCacheReads).toBe(true);
      expect(DEFAULT_PRICE_BOOK["claude-sonnet-4-6"]?.promptIncludesCacheReads).toBe(false);
    });
  });

  describe("cost telemetry", () => {
    type Attr = { key: string; value: Record<string, unknown> };
    type Span = { name: string; attributes: Attr[] };
    const find = (span: Span, key: string): Record<string, unknown> | undefined =>
      span.attributes.find((a) => a.key === key)?.value;

    const traceOf = async (
      nodes: Array<{ id: string; model?: string; usage?: TokenUsage }>,
      priceBook?: PriceBook
    ): Promise<Span[]> => {
      const captured: unknown[] = [];
      const fetchImpl: OtlpFetch = async (_url, init) => {
        captured.push(JSON.parse(init.body));
        return { ok: true, status: 200 };
      };
      // A fake app that replays lifecycle events, so the test needs no native engine.
      let listener: ((event: RunEvent) => void) | undefined;
      const app = {
        onEvent: (cb: (event: RunEvent) => void) => {
          listener = cb;
          return () => {};
        }
      } as unknown as CompiledGraph;
      const stop = exportTracesToOtlp(app, {
        endpoint: "http://otel.local/v1/traces",
        fetchImpl,
        ...(priceBook === undefined ? {} : { priceBook })
      });
      const emit = (event: Record<string, unknown>): void =>
        listener?.({ runId: "run_cost", timestamp: "2026-01-01T00:00:00.000Z", ...event } as RunEvent);
      emit({ type: "run_started" });
      for (const n of nodes) {
        emit({ type: "node_started", nodeId: n.id });
        emit({
          type: "node_completed",
          nodeId: n.id,
          output:
            n.usage === undefined
              ? { r: "plain" }
              : { r: { usage: n.usage, ...(n.model === undefined ? {} : { model: n.model }) } }
        });
      }
      emit({ type: "run_completed" });
      stop();
      const body = captured[0] as { resourceSpans: [{ scopeSpans: [{ spans: Span[] }] }] };
      return body.resourceSpans[0].scopeSpans[0].spans;
    };

    const usage: TokenUsage = { promptTokens: 1e6, completionTokens: 0 };

    it("omits ailu.cost.usd for an unknown model and says so with a status", async () => {
      const spans = await traceOf([{ id: "a", model: "who-knows", usage }]);
      const a = spans.find((s) => s.name === "a")!;
      expect(find(a, "ailu.cost.usd")).toBeUndefined();
      expect(find(a, "ailu.cost.amount")).toBeUndefined();
      expect(find(a, "ailu.cost.status")).toEqual({ stringValue: "unknown" });
      expect(find(a, "ailu.cost.unpriced_tokens")).toEqual({ intValue: 1e6 });
    });

    it("emits a measured zero cost, with status known", async () => {
      const spans = await traceOf([{ id: "a", model: "free", usage }], {
        free: { inPerMtok: 0, outPerMtok: 0 }
      });
      const a = spans.find((s) => s.name === "a")!;
      expect(find(a, "ailu.cost.usd")).toEqual({ intValue: 0 });
      expect(find(a, "ailu.cost.amount")).toEqual({ intValue: 0 });
      expect(find(a, "ailu.cost.status")).toEqual({ stringValue: "known" });
    });

    it("emits an unknown status for usage without a model", async () => {
      const spans = await traceOf([{ id: "a", usage }]);
      const a = spans.find((s) => s.name === "a")!;
      expect(find(a, "ailu.cost.status")).toEqual({ stringValue: "unknown" });
      expect(find(a, "ailu.cost.usd")).toBeUndefined();
    });

    it("summarises the run on the root span, incomplete when a node is unknown", async () => {
      const spans = await traceOf([
        { id: "a", model: "claude-opus-4-8", usage },
        { id: "b", model: "who-knows", usage }
      ]);
      const root = spans.find((s) => s.name === "run")!;
      expect(find(root, "ailu.run.cost.status")).toEqual({ stringValue: "estimated" });
      expect(find(root, "ailu.run.cost.complete")).toEqual({ boolValue: false });
      expect(find(root, "ailu.run.cost.amount")).toEqual({ intValue: 15 });
      expect(find(root, "ailu.run.cost.coverage")).toEqual({ doubleValue: 0.5 });
      expect(find(root, "ailu.run.cost.unpriced_tokens")).toEqual({ intValue: 1e6 });
      expect(find(root, "ailu.run.cost.currency")).toEqual({ stringValue: "USD" });
    });

    it("does not count twice the usage a subgraph node forwards from its child run", async () => {
      const captured: unknown[] = [];
      const fetchImpl: OtlpFetch = async (_url, init) => {
        captured.push(JSON.parse(init.body));
        return { ok: true, status: 200 };
      };
      let listener: ((event: RunEvent) => void) | undefined;
      const app = {
        onEvent: (cb: (event: RunEvent) => void) => {
          listener = cb;
          return () => {};
        }
      } as unknown as CompiledGraph;
      const stop = exportTracesToOtlp(app, { endpoint: "http://otel.local/v1/traces", fetchImpl });
      const emit = (runId: string, event: Record<string, unknown>): void =>
        listener?.({ runId, timestamp: "2026-01-01T00:00:00.000Z", ...event } as RunEvent);
      const result = { usage: { promptTokens: 1e6, completionTokens: 0 }, model: "claude-opus-4-8" };
      emit("run_sub", { type: "run_started" });
      emit("run_sub", { type: "node_started", nodeId: "child" });
      emit("run_sub:child", { type: "node_started", nodeId: "agent" });
      emit("run_sub:child", { type: "node_completed", nodeId: "agent", output: { r: result } });
      emit("run_sub", { type: "node_completed", nodeId: "child", output: { r: result } });
      emit("run_sub", { type: "run_completed" });
      stop();
      const body = captured[0] as { resourceSpans: [{ scopeSpans: [{ spans: Span[] }] }] };
      const spans = body.resourceSpans[0].scopeSpans[0].spans;
      const root = spans.find((s) => s.name === "run")!;
      expect(find(root, "ailu.run.cost.amount")).toEqual({ intValue: 15 });
      const forwarding = spans.find((s) => s.name === "child")!;
      expect(find(forwarding, "ailu.cost.usd")).toBeUndefined();
      expect(find(forwarding, "ailu.cost.status")).toBeUndefined();
    });

    it("counts every concurrent child run of a mapped subgraph, not only the first", async () => {
      const captured: unknown[] = [];
      const fetchImpl: OtlpFetch = async (_url, init) => {
        captured.push(JSON.parse(init.body));
        return { ok: true, status: 200 };
      };
      let listener: ((event: RunEvent) => void) | undefined;
      const app = {
        onEvent: (cb: (event: RunEvent) => void) => {
          listener = cb;
          return () => {};
        }
      } as unknown as CompiledGraph;
      const stop = exportTracesToOtlp(app, { endpoint: "http://otel.local/v1/traces", fetchImpl });
      const emit = (runId: string, event: Record<string, unknown>): void =>
        listener?.({ runId, timestamp: "2026-01-01T00:00:00.000Z", ...event } as RunEvent);
      const result = { usage: { promptTokens: 1e6, completionTokens: 0 }, model: "claude-opus-4-8" };
      emit("run_map", { type: "run_started" });
      emit("run_map", { type: "node_started", nodeId: "each" });
      // Both children start the same node before either completes.
      emit("run_map:each:0", { type: "node_started", nodeId: "agent" });
      emit("run_map:each:1", { type: "node_started", nodeId: "agent" });
      emit("run_map:each:0", { type: "node_completed", nodeId: "agent", output: { r: result } });
      emit("run_map:each:1", { type: "node_completed", nodeId: "agent", output: { r: result } });
      emit("run_map", { type: "node_completed", nodeId: "each", output: { r: result } });
      emit("run_map", { type: "run_completed" });
      stop();
      const body = captured[0] as { resourceSpans: [{ scopeSpans: [{ spans: Span[] }] }] };
      const spans = body.resourceSpans[0].scopeSpans[0].spans;
      const root = spans.find((s) => s.name === "run")!;
      expect(find(root, "ailu.run.cost.amount")).toEqual({ intValue: 30 });
      expect(spans.filter((s) => s.name === "agent")).toHaveLength(2);
    });

    it("leaves a non-finite token count out of the span and says the cost is unknown", async () => {
      const spans = await traceOf([
        { id: "a", model: "claude-opus-4-8", usage: { promptTokens: Number.NaN, completionTokens: 5 } }
      ]);
      const a = spans.find((s) => s.name === "a")!;
      expect(find(a, "gen_ai.usage.input_tokens")).toBeUndefined();
      expect(find(a, "gen_ai.usage.output_tokens")).toEqual({ intValue: 5 });
      expect(find(a, "ailu.cost.status")).toEqual({ stringValue: "unknown" });
      expect(find(a, "ailu.cost.usd")).toBeUndefined();
    });

    it("leaves the root span without cost attributes when no node had usage", async () => {
      const spans = await traceOf([{ id: "plain" }]);
      const root = spans.find((s) => s.name === "run");
      expect(root?.attributes.some((a) => a.key.startsWith("ailu.run.cost."))).toBeFalsy();
    });
  });
});
