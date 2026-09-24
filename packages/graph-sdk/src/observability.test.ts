import { describe, expect, it } from "vitest";

import {
  buildOtlpPayload,
  computeCost,
  createGraph,
  exportTracesToOtlp,
  type OtlpFetch,
  type RunId
} from "./index.js";

describe("@ailu/graph-sdk — observability (ADR 0028 phase 7)", () => {
  it("computeCost prices usage against the book (unknown model = 0)", () => {
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
});
