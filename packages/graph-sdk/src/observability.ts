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

import type { RunEvent } from "@ailu/graph-runtime";

import type { CompiledGraph } from "./compiled-graph.js";

// ── Cost ────────────────────────────────────────────────────────────────────────────────────

/** Price of a model, in US dollars per 1,000,000 tokens. */
export type ModelPrice = { inPerMtok: number; outPerMtok: number };

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
 * A small default price book ($/Mtok, indicative — prices drift; supply your own to override).
 * Keys are model ids; an unknown model costs `0` (and is reported as such, never guessed).
 */
export const DEFAULT_PRICE_BOOK: PriceBook = {
  "claude-opus-4-8": { inPerMtok: 15, outPerMtok: 75 },
  "claude-sonnet-4-6": { inPerMtok: 3, outPerMtok: 15 },
  "claude-haiku-4-5": { inPerMtok: 1, outPerMtok: 5 },
  "gpt-4o": { inPerMtok: 2.5, outPerMtok: 10 },
  "gpt-4o-mini": { inPerMtok: 0.15, outPerMtok: 0.6 },
  "gemini-2.5-pro": { inPerMtok: 1.25, outPerMtok: 10 },
  "gemini-2.5-flash": { inPerMtok: 0.3, outPerMtok: 2.5 },
  "mistral-large-latest": { inPerMtok: 2, outPerMtok: 6 }
};

/**
 * Compute the US-dollar cost of a usage record against a price book. Cache-read tokens (when
 * priced separately) are not modelled here — they fold into prompt tokens at the input rate, a
 * conservative upper bound. Returns `0` for an unknown model (never a guess).
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
  /** Price book for the `ailu.cost.usd` span attribute. Default {@link DEFAULT_PRICE_BOOK}. */
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
  type RunTrace = { open: Map<string, OpenSpan>; finished: FinishedSpan[]; startNano: string };
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
    const all: FinishedSpan[] = [
      {
        spanId: hexId(`${runId}#run`, 8),
        name: "run",
        startNano: trace.startNano || endNano,
        endNano,
        status,
        nodeId: undefined,
        attributes: { "ailu.run_id": runId }
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

  const unsubscribe = app.onEvent((event: RunEvent) => {
    const runId = traceIdOf(String(event.runId));
    switch (event.type) {
      case "node_started": {
        let trace = traces.get(runId);
        if (trace === undefined) {
          trace = { open: new Map(), finished: [], startNano: toNano(event.timestamp) };
          traces.set(runId, trace);
        }
        trace.open.set(String(event.nodeId), {
          spanId: hexId(`${runId}#${String(event.nodeId)}`, 8),
          name: String(event.nodeId),
          startNano: toNano(event.timestamp),
          nodeId: String(event.nodeId)
        });
        break;
      }
      case "node_completed": {
        const trace = traces.get(runId);
        const span = trace?.open.get(String(event.nodeId));
        if (trace !== undefined && span !== undefined) {
          trace.open.delete(String(event.nodeId));
          trace.finished.push({
            ...span,
            endNano: toNano(event.timestamp),
            status: 1,
            attributes: nodeAttributes(runId, String(event.nodeId), event.output, priceBook)
          });
        }
        break;
      }
      case "node_failed": {
        const trace = traces.get(runId);
        const span = trace?.open.get(String(event.nodeId));
        if (trace !== undefined && span !== undefined) {
          trace.open.delete(String(event.nodeId));
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

/** Span attributes for a completed node: run/node ids, plus token usage + cost for agent nodes. */
function nodeAttributes(
  runId: string,
  nodeId: string,
  output: unknown,
  priceBook: PriceBook
): Record<string, string | number | boolean> {
  const attrs: Record<string, string | number | boolean> = {
    "ailu.run_id": runId,
    "ailu.node_id": nodeId
  };
  // An agent node's output (in its output channel) is an AgentResult carrying `usage`.
  const result = extractAgentResult(output);
  if (result?.usage !== undefined) {
    const u = result.usage;
    attrs["gen_ai.usage.input_tokens"] = u.promptTokens;
    attrs["gen_ai.usage.output_tokens"] = u.completionTokens;
    if (result.model !== undefined) {
      attrs["gen_ai.response.model"] = result.model;
      attrs["ailu.cost.usd"] = computeCost(u, result.model, priceBook);
    }
  }
  return attrs;
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
