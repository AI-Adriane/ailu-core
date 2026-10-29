/**
 * Observability (ADR 0028 phase 7): turn a run's lifecycle events into **OTLP spans** and ship
 * them to any OpenTelemetry endpoint — LangSmith, Langfuse, Phoenix, Datadog, Grafana, … — plus
 * a **cost** mapping from the token usage the engine now reports on `AgentResult.usage`.
 *
 * This lives in the SDK, not the engine: the engine already emits a `RunEvent` per node
 * transition (the audit journal) and `AgentResult.usage` per agent — observability is a *read
 * view* over that, an integration concern. The engine stays lean; nothing here can alter a run.
 *
 * ```ts
 * const stop = exportTracesToOtlp(app, { endpoint: process.env.OTEL_EXPORTER_OTLP_ENDPOINT });
 * await app.run({ … });
 * stop(); // flushes + unsubscribes (also auto-flushes on run_completed / run_failed)
 * ```
 */

import type { RunEvent } from "@ailu-ai/graph-runtime";

import type { CompiledGraph } from "./compiled-graph.js";

// ── Cost ────────────────────────────────────────────────────────────────────────────────────

/**
 * Price of a model, per 1,000,000 tokens (US dollars unless `currency` says otherwise).
 * The optional fields are additive: a price book written before they existed keeps working.
 */
export type ModelPrice = {
  inPerMtok: number;
  outPerMtok: number;
  /** Rate for cache-read tokens. Absent: they are counted at `inPerMtok` and the cost is `estimated`. */
  cacheReadPerMtok?: number;
  /** Rate for cache-write tokens. Absent: they are counted at `inPerMtok` and the cost is `estimated`. */
  cacheWritePerMtok?: number;
  /** Currency of every rate of this entry. Default `"USD"`. */
  currency?: string;
  /**
   * Whether the provider counts cache-read tokens inside `promptTokens` (`true`: OpenAI-style and
   * Gemini-style usage) or apart from it (`false`: Anthropic-style usage). Absent: unknown, so a
   * usage with cache-read tokens is `estimated` and counted as apart from `promptTokens`.
   */
  promptIncludesCacheReads?: boolean;
  /** Free-text provenance of the rate (a price page, a contract, a date). */
  source?: string;
  /** `true`: the rate is a rough guide, so a cost computed from it is at best `estimated`. */
  indicative?: boolean;
};

/** A price table keyed by model id (the `AgentResult.usage` carries the model on its calls). */
export type PriceBook = Record<string, ModelPrice>;

/** Token usage shape (matches `AgentResult.usage` / the engine's `LlmUsage`). */
export type TokenUsage = {
  promptTokens: number;
  completionTokens: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
};

/**
 * A small default price book ($/Mtok). It is **indicative**: prices drift, so every entry is
 * marked `indicative: true` and `estimateCost` reports its results as `estimated`, never `known`.
 * Supply your own book to override. Keys are model ids; a model that is not in the book has an
 * `unknown` cost (never a guess, never `0`).
 */
export const DEFAULT_PRICE_BOOK: PriceBook = {
  "claude-opus-4-8": { inPerMtok: 15, outPerMtok: 75, promptIncludesCacheReads: false, indicative: true },
  "claude-sonnet-4-6": { inPerMtok: 3, outPerMtok: 15, promptIncludesCacheReads: false, indicative: true },
  "claude-haiku-4-5": { inPerMtok: 1, outPerMtok: 5, promptIncludesCacheReads: false, indicative: true },
  "gpt-4o": { inPerMtok: 2.5, outPerMtok: 10, promptIncludesCacheReads: true, indicative: true },
  "gpt-4o-mini": { inPerMtok: 0.15, outPerMtok: 0.6, promptIncludesCacheReads: true, indicative: true },
  "gemini-2.5-pro": { inPerMtok: 1.25, outPerMtok: 10, promptIncludesCacheReads: true, indicative: true },
  "gemini-2.5-flash": { inPerMtok: 0.3, outPerMtok: 2.5, promptIncludesCacheReads: true, indicative: true },
  "mistral-large-latest": { inPerMtok: 2, outPerMtok: 6, indicative: true }
};

/**
 * Compute the US-dollar cost of a usage record against a price book. Cache-read tokens (when
 * priced separately) are not modelled here — they fold into prompt tokens at the input rate, a
 * conservative upper bound; cache-write tokens are not counted.
 *
 * Returns `0` for a model that is missing from the book, so **it cannot tell an unknown model
 * from a real zero cost**. Prefer {@link estimateCost}, which says when a cost is unknown or
 * estimated and how much of the usage was priced.
 */
export function computeCost(
  usage: TokenUsage,
  model: string,
  book: PriceBook = DEFAULT_PRICE_BOOK
): number {
  const price = book[model];
  if (price === undefined) {
    return 0;
  }
  const inTokens = usage.promptTokens + (usage.cacheReadTokens ?? 0);
  return (inTokens / 1e6) * price.inPerMtok + (usage.completionTokens / 1e6) * price.outPerMtok;
}

// ── Cost with a status ──────────────────────────────────────────────────────────────────────

/**
 * How far a cost can be trusted: `known` (every token priced with an explicit, non-indicative
 * rate), `estimated` (priced, but with an indicative rate or an approximated cache rate), or
 * `unknown` (nothing could be priced).
 */
export type CostStatus = "known" | "estimated" | "unknown";

/** Why a cost is not `known`. */
export type CostReason =
  | "modelNotInBook"
  | "indicativePrice"
  | "cacheApproximated"
  | "missingUsage"
  | "missingModel"
  | "invalidPrice"
  | "invalidUsage";

/** The cost of one usage record, with its status and how much of the usage it covers. */
export type CostResult = {
  status: CostStatus;
  /** `null` when `status` is `"unknown"`. Never `0` for an unknown cost. */
  amount: number | null;
  currency: string;
  /** Share of the tokens that were priced, in [0, 1]. */
  coverage: number;
  /** Tokens that could not be priced. */
  unpricedTokens: number;
  /** Tokens that were priced. Lets {@link aggregateCosts} weight results by size. */
  pricedTokens: number;
  reasons: CostReason[];
};

const DEFAULT_CURRENCY = "USD";

const totalTokens = (u: TokenUsage): number =>
  u.promptTokens + u.completionTokens + (u.cacheReadTokens ?? 0) + (u.cacheWriteTokens ?? 0);

const validCount = (n: number | undefined): boolean =>
  n === undefined || (Number.isFinite(n) && n >= 0);

// The prompt and completion counts are required: a record without them (or with a count that is
// not a finite number >= 0) cannot be priced. Only the cache counts are optional.
const usageIsValid = (u: TokenUsage): boolean =>
  u.promptTokens !== undefined &&
  u.completionTokens !== undefined &&
  validCount(u.promptTokens) &&
  validCount(u.completionTokens) &&
  validCount(u.cacheReadTokens) &&
  validCount(u.cacheWriteTokens);

const validRate = (rate: number | undefined, required = false): boolean =>
  rate === undefined ? !required : Number.isFinite(rate) && rate >= 0;

const unknownCost = (unpricedTokens: number, reasons: CostReason[]): CostResult => ({
  status: "unknown",
  amount: null,
  currency: DEFAULT_CURRENCY,
  coverage: 0,
  unpricedTokens,
  pricedTokens: 0,
  reasons
});

/**
 * Estimate the cost of a usage record and say how far it can be trusted.
 *
 * - `unknown`: the model, the usage or the book entry is missing, or a rate is invalid (negative,
 *   `NaN`, infinite). `amount` is `null` — an unknown cost is never `0`.
 * - `estimated`: the entry is `indicative`, or cache tokens are present without their own rate
 *   (they are then counted at the input rate), or cache reads are present and the entry does not
 *   say whether `promptTokens` includes them (`promptIncludesCacheReads`).
 * - `known`: every token type present has an explicit rate on a non-indicative entry.
 *
 * A model is "absent" when the book has no entry for it; an entry with rates of `0` is a valid
 * free price. Without `book`, {@link DEFAULT_PRICE_BOOK} is used, whose entries are indicative.
 */
export function estimateCost(
  usage: TokenUsage | undefined,
  model: string | undefined,
  book: PriceBook = DEFAULT_PRICE_BOOK
): CostResult {
  const tokens = usage === undefined || !usageIsValid(usage) ? 0 : totalTokens(usage);
  const reasons: CostReason[] = [];
  if (usage === undefined) reasons.push("missingUsage");
  if (model === undefined) reasons.push("missingModel");
  if (usage === undefined || model === undefined) return unknownCost(tokens, reasons);

  // A token count must be a finite, non-negative number: anything else cannot be priced, and
  // must not turn into a `known` cost with an amount that is `NaN` or negative.
  if (!usageIsValid(usage)) return unknownCost(0, ["invalidUsage"]);
  const price = Object.prototype.hasOwnProperty.call(book, model) ? book[model] : undefined;
  if (price === undefined) return unknownCost(tokens, ["modelNotInBook"]);
  if (typeof price !== "object" || price === null) return unknownCost(tokens, ["invalidPrice"]);
  if (
    !validRate(price.inPerMtok, true) ||
    !validRate(price.outPerMtok, true) ||
    !validRate(price.cacheReadPerMtok) ||
    !validRate(price.cacheWritePerMtok)
  ) {
    return unknownCost(tokens, ["invalidPrice"]);
  }

  const cacheRead = usage.cacheReadTokens ?? 0;
  const cacheWrite = usage.cacheWriteTokens ?? 0;
  const readRate = price.cacheReadPerMtok ?? price.inPerMtok;
  const writeRate = price.cacheWritePerMtok ?? price.inPerMtok;
  // When the provider counts cache reads inside `promptTokens`, they are billed once, at the
  // cache-read rate: take them out of the prompt tokens charged at the input rate.
  const inclusive = price.promptIncludesCacheReads === true;
  const promptBilled = inclusive ? Math.max(usage.promptTokens - cacheRead, 0) : usage.promptTokens;
  const amount =
    (promptBilled / 1e6) * price.inPerMtok +
    (usage.completionTokens / 1e6) * price.outPerMtok +
    (cacheRead / 1e6) * readRate +
    (cacheWrite / 1e6) * writeRate;

  // Huge counts or rates can overflow to Infinity: such an amount is not a cost.
  if (!Number.isFinite(amount)) return unknownCost(tokens, ["invalidUsage"]);
  if (price.indicative === true) reasons.push("indicativePrice");
  if (
    (cacheRead > 0 && price.cacheReadPerMtok === undefined) ||
    (cacheWrite > 0 && price.cacheWritePerMtok === undefined) ||
    // The usage does not say whether the prompt count includes the cache reads, and the price
    // does not either: the amount assumes they are apart and may double count them.
    (cacheRead > 0 && price.promptIncludesCacheReads === undefined) ||
    // More cache reads than prompt tokens cannot be inclusive: the usage is inconsistent.
    (inclusive && cacheRead > usage.promptTokens)
  ) {
    reasons.push("cacheApproximated");
  }
  return {
    status: reasons.length === 0 ? "known" : "estimated",
    amount,
    currency: price.currency ?? DEFAULT_CURRENCY,
    coverage: 1,
    unpricedTokens: 0,
    // Cache reads counted inside `promptTokens` are one set of tokens, not two.
    pricedTokens: inclusive ? tokens - Math.min(cacheRead, usage.promptTokens) : tokens,
    reasons
  };
}

/** The cost of several calls, summed over the measured part only. */
export type CostSummary = {
  status: CostStatus;
  /** Sum of the measured part. `null` when nothing was measured. */
  amount: number | null;
  currency: string;
  /** Priced tokens over all tokens, in [0, 1]. */
  coverage: number;
  /** `true` only when every token was priced, in one currency, with no unknown call. */
  complete: boolean;
  unpricedTokens: number;
  calls: { known: number; estimated: number; unknown: number };
};

/**
 * Sum {@link CostResult}s without hiding what is missing.
 *
 * - `amount` adds the measured part only: an unknown call adds nothing (not a `0`) and lowers
 *   `coverage` instead. An empty list is not `0` and not complete: it is `unknown`.
 * - Pass the results **in event-emission order**: float addition is not associative, so another
 *   order can differ in the last digit. Compare totals with a tolerance.
 * - Only one currency is summed: that of the first measured result. A result in another currency
 *   is not added; its tokens count as `unpricedTokens` and `complete` is `false`.
 * - `status` is `unknown` if nothing was measured, `known` only if every result is `known` and
 *   the total is complete, `estimated` otherwise.
 */
export function aggregateCosts(results: CostResult[]): CostSummary {
  const calls = { known: 0, estimated: 0, unknown: 0 };
  let currency: string | undefined;
  let amount: number | null = null;
  let priced = 0;
  let unpriced = 0;
  let foreign = false;
  for (const r of results) {
    calls[r.status] += 1;
    unpriced += Number.isFinite(r.unpricedTokens) ? r.unpricedTokens : 0;
    if (r.amount === null) continue;
    const pricedTokens = Number.isFinite(r.pricedTokens) ? r.pricedTokens : 0;
    currency ??= r.currency;
    if (r.currency !== currency) {
      foreign = true;
      unpriced += pricedTokens;
      continue;
    }
    amount = (amount ?? 0) + r.amount;
    priced += pricedTokens;
  }
  const all = priced + unpriced;
  const measured = amount !== null;
  const coverage = !measured ? 0 : all === 0 ? 1 : priced / all;
  const complete = measured && coverage === 1 && calls.unknown === 0 && !foreign;
  const status: CostStatus = !measured
    ? "unknown"
    : complete && calls.estimated === 0
      ? "known"
      : "estimated";
  return {
    status,
    amount,
    currency: currency ?? DEFAULT_CURRENCY,
    coverage,
    complete,
    unpricedTokens: unpriced,
    calls
  };
}

// ── OTLP export ─────────────────────────────────────────────────────────────────────────────

/** Minimal `fetch` shape, so the exporter is injectable for tests (no real network). */
export type OtlpFetch = (
  url: string,
  init: { method: string; headers: Record<string, string>; body: string }
) => Promise<{ ok: boolean; status: number }>;

export type OtelExporterOptions = {
  /** OTLP/HTTP traces endpoint. Defaults to `AILU_OTEL_EXPORTER_URL` env. */
  endpoint?: string;
  /** `service.name` resource attribute. Default `"ailu"`. */
  serviceName?: string;
  /** Extra headers (e.g. an API key for LangSmith / Langfuse). */
  headers?: Record<string, string>;
  /** Price book for the `ailu.cost.*` span attributes. Default {@link DEFAULT_PRICE_BOOK}. */
  priceBook?: PriceBook;
  /** Injected `fetch` (tests). Defaults to the global `fetch`. */
  fetchImpl?: OtlpFetch;
};

type OpenSpan = { spanId: string; name: string; startNano: string; nodeId?: string };
type FinishedSpan = OpenSpan & {
  endNano: string;
  status: 0 | 1 | 2; // UNSET | OK | ERROR
  attributes: Record<string, string | number | boolean>;
};

const toNano = (iso: string): string => {
  const ms = Date.parse(iso);
  return `${(Number.isNaN(ms) ? 0 : ms) * 1_000_000}`;
};

/** Deterministic hex id of `bytes` bytes from a seed string (FNV-1a, repeated). Not crypto. */
const hexId = (seed: string, bytes: number): string => {
  let out = "";
  let h = 0x811c9dc5;
  for (let i = 0; out.length < bytes * 2; i += 1) {
    for (const ch of `${seed}#${i}`) {
      h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193) >>> 0;
    }
    out += h.toString(16).padStart(8, "0");
  }
  return out.slice(0, bytes * 2);
};

const attrValue = (v: string | number | boolean): Record<string, unknown> => {
  if (typeof v === "number") return Number.isInteger(v) ? { intValue: v } : { doubleValue: v };
  if (typeof v === "boolean") return { boolValue: v };
  return { stringValue: v };
};

/**
 * Build the OTLP/HTTP-JSON `traces` payload for one run: a root span for the run plus one span
 * per completed/failed node, each tagged with `ailu.run_id` / `ailu.node_id`, and agent
 * nodes additionally with token usage + computed cost.
 */
export function buildOtlpPayload(
  runId: string,
  spans: FinishedSpan[],
  serviceName: string
): string {
  const traceId = hexId(runId, 16);
  return JSON.stringify({
    resourceSpans: [
      {
        resource: { attributes: [{ key: "service.name", value: { stringValue: serviceName } }] },
        scopeSpans: [
          {
            scope: { name: "ailu" },
            spans: spans.map((s) => ({
              traceId,
              spanId: s.spanId,
              name: s.name,
              kind: 1,
              startTimeUnixNano: s.startNano,
              endTimeUnixNano: s.endNano,
              status: { code: s.status },
              attributes: Object.entries(s.attributes).map(([key, value]) => ({
                key,
                value: attrValue(value)
              }))
            }))
          }
        ]
      }
    ]
  });
}

/**
 * Subscribe to `app`'s run-lifecycle events and export each run as an OTLP trace. Returns an
 * unsubscribe fn; the run is flushed automatically on `run_completed` / `run_failed` (and on the
 * returned fn). **Fail-open**: an export error is swallowed (best-effort observability never
 * fails a run). A missing endpoint disables export (the returned fn is a no-op).
 */
export function exportTracesToOtlp(
  app: CompiledGraph,
  options: OtelExporterOptions = {}
): () => void {
  const endpoint = options.endpoint ?? process.env.AILU_OTEL_EXPORTER_URL;
  if (endpoint === undefined || endpoint === "") {
    return () => {};
  }
  const serviceName = options.serviceName ?? "ailu";
  const priceBook = options.priceBook ?? DEFAULT_PRICE_BOOK;
  const doFetch: OtlpFetch =
    options.fetchImpl ??
    ((url, init) => fetch(url, init).then((r) => ({ ok: r.ok, status: r.status })));

  // One trace per root run. The exporter subscribes to the app's shared event feed, which carries
  // the events of every in-flight run, so spans are grouped by run id; a child run's events
  // (`<rootRunId>:<nodeId>[:<index>]`) are folded into the root run's trace, as before.
  type RunTrace = {
    open: Map<string, OpenSpan>;
    finished: FinishedSpan[];
    costs: CostResult[];
    /** Ids of the child runs (`<root>:<node>…`) seen so far, to spot nodes that forward them. */
    childRuns: Set<string>;
    startNano: string;
  };
  const traces = new Map<string, RunTrace>();

  const traceIdOf = (eventRunId: string): string => {
    if (traces.has(eventRunId)) return eventRunId;
    for (const rootId of traces.keys()) {
      if (eventRunId.startsWith(`${rootId}:`)) return rootId;
    }
    return eventRunId;
  };

  const flush = (runId: string, endNano: string, status: 0 | 1 | 2): void => {
    const trace = traces.get(runId);
    if (trace === undefined) return;
    traces.delete(runId);
    const runAttributes: Record<string, string | number | boolean> = { "ailu.run_id": runId };
    if (trace.costs.length > 0) {
      // Costs are in node-completion order, the order the events were produced.
      const summary = aggregateCosts(trace.costs);
      runAttributes["ailu.run.cost.status"] = summary.status;
      if (summary.amount !== null) runAttributes["ailu.run.cost.amount"] = summary.amount;
      runAttributes["ailu.run.cost.currency"] = summary.currency;
      runAttributes["ailu.run.cost.coverage"] = summary.coverage;
      runAttributes["ailu.run.cost.complete"] = summary.complete;
      runAttributes["ailu.run.cost.unpriced_tokens"] = summary.unpricedTokens;
    }
    const all: FinishedSpan[] = [
      {
        spanId: hexId(`${runId}#run`, 8),
        name: "run",
        startNano: trace.startNano || endNano,
        endNano,
        status,
        nodeId: undefined,
        attributes: runAttributes
      },
      ...trace.finished
    ];
    const body = buildOtlpPayload(runId, all, serviceName);
    void doFetch(endpoint, {
      method: "POST",
      headers: { "content-type": "application/json", ...(options.headers ?? {}) },
      body
    }).catch(() => {
      /* fail-open: observability never breaks a run */
    });
  };

  const spanKey = (eventRunId: unknown, nodeId: unknown): string =>
    `${String(eventRunId)}\u0000${String(nodeId)}`;

  const unsubscribe = app.onEvent((event: RunEvent) => {
    const runId = traceIdOf(String(event.runId));
    if (String(event.runId) !== runId) traces.get(runId)?.childRuns.add(String(event.runId));
    switch (event.type) {
      case "node_started": {
        let trace = traces.get(runId);
        if (trace === undefined) {
          trace = {
            open: new Map(),
            finished: [],
            costs: [],
            childRuns: new Set(),
            startNano: toNano(event.timestamp)
          };
          traces.set(runId, trace);
        }
        // Open spans are keyed by run AND node: concurrent child runs (a mapped subgraph) run the
        // same node ids, and must not overwrite each other's span.
        trace.open.set(spanKey(event.runId, event.nodeId), {
          spanId: hexId(`${String(event.runId)}#${String(event.nodeId)}`, 8),
          name: String(event.nodeId),
          startNano: toNano(event.timestamp),
          nodeId: String(event.nodeId)
        });
        break;
      }
      case "node_completed": {
        const trace = traces.get(runId);
        const span = trace?.open.get(spanKey(event.runId, event.nodeId));
        if (trace !== undefined && span !== undefined) {
          trace.open.delete(spanKey(event.runId, event.nodeId));
          // A subgraph or fan-out node forwards its child runs' channels, usage included: that
          // usage was already counted by the child runs' own agent nodes, so it is not counted
          // again on the node that carries it. (Usage such a node incurs itself, besides its
          // children's, is not counted either: its output cannot be split.)
          const ownChildren = `${String(event.runId)}:${String(event.nodeId)}`;
          const forwarded = [...trace.childRuns].some(
            (child) => child === ownChildren || child.startsWith(`${ownChildren}:`)
          );
          // The child runs of this node are done with: forget them, so the set stays small.
          for (const child of [...trace.childRuns]) {
            if (child === ownChildren || child.startsWith(`${ownChildren}:`)) {
              trace.childRuns.delete(child);
            }
          }
          const node = nodeAttributes(
            runId,
            String(event.nodeId),
            forwarded ? undefined : event.output,
            priceBook
          );
          if (node.cost !== undefined) trace.costs.push(node.cost);
          trace.finished.push({
            ...span,
            endNano: toNano(event.timestamp),
            status: 1,
            attributes: node.attributes
          });
        }
        break;
      }
      case "node_failed": {
        const trace = traces.get(runId);
        const span = trace?.open.get(spanKey(event.runId, event.nodeId));
        if (trace !== undefined && span !== undefined) {
          trace.open.delete(spanKey(event.runId, event.nodeId));
          trace.finished.push({
            ...span,
            endNano: toNano(event.timestamp),
            status: 2,
            attributes: { "ailu.run_id": runId, "ailu.node_id": String(event.nodeId), error: String(event.error) }
          });
        }
        break;
      }
      // Only the root run's terminal event closes its trace; a child run completing mid-way
      // must not flush the parent's spans early.
      case "run_completed":
        if (String(event.runId) === runId) flush(runId, toNano(event.timestamp), 1);
        break;
      case "run_failed":
        if (String(event.runId) === runId) flush(runId, toNano(event.timestamp), 2);
        break;
      default:
        break;
    }
  });

  return () => {
    const now = `${Date.now() * 1_000_000}`;
    for (const runId of [...traces.keys()]) {
      flush(runId, now, 0);
    }
    unsubscribe();
  };
}

/**
 * Span attributes for a completed node: run/node ids, plus token usage + cost for agent nodes.
 * The cost result is returned too, so the root span can summarise the run.
 *
 * `ailu.cost.usd` is present only when the cost is measured (and in USD): its absence means
 * the cost is unknown, never that the call was free.
 */
function nodeAttributes(
  runId: string,
  nodeId: string,
  output: unknown,
  priceBook: PriceBook
): { attributes: Record<string, string | number | boolean>; cost?: CostResult } {
  const attrs: Record<string, string | number | boolean> = {
    "ailu.run_id": runId,
    "ailu.node_id": nodeId
  };
  // An agent node's output (in its output channel) is an AgentResult carrying `usage`.
  const result = extractAgentResult(output);
  if (result?.usage === undefined) return { attributes: attrs };
  const u = result.usage;
  // Token counts that are not finite numbers have no JSON form: leave them out of the span.
  if (Number.isFinite(u.promptTokens)) attrs["gen_ai.usage.input_tokens"] = u.promptTokens;
  if (Number.isFinite(u.completionTokens)) attrs["gen_ai.usage.output_tokens"] = u.completionTokens;
  if (result.model !== undefined) attrs["gen_ai.response.model"] = result.model;
  const cost = estimateCost(u, result.model, priceBook);
  attrs["ailu.cost.status"] = cost.status;
  attrs["ailu.cost.currency"] = cost.currency;
  attrs["ailu.cost.coverage"] = cost.coverage;
  attrs["ailu.cost.unpriced_tokens"] = cost.unpricedTokens;
  if (cost.amount !== null) {
    attrs["ailu.cost.amount"] = cost.amount;
    if (cost.currency === DEFAULT_CURRENCY) attrs["ailu.cost.usd"] = cost.amount;
  }
  return { attributes: attrs, cost };
}

/** Pull the first `AgentResult`-shaped value (one carrying `usage`) out of a node output map. */
function extractAgentResult(
  output: unknown
): { usage?: TokenUsage; model?: string } | undefined {
  if (output === null || typeof output !== "object") return undefined;
  for (const value of Object.values(output as Record<string, unknown>)) {
    if (value !== null && typeof value === "object" && "usage" in value) {
      const usage = (value as { usage?: unknown }).usage;
      if (usage !== null && typeof usage === "object" && "promptTokens" in usage) {
        return value as { usage?: TokenUsage; model?: string };
      }
    }
  }
  return undefined;
}
