import assert from "node:assert/strict";

import { createGraph, exportTracesToOtlp, type OtlpFetch } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "pipeline" })
  .node("a", async () => ({}))
  .node("b", async () => ({}))
  .edge("a", "b")
  .compile();

// Offline stand-in for the collector, so this file runs in tests.
const sent: unknown[] = [];
const collector: OtlpFetch = async (_url, init) => {
  sent.push(JSON.parse(String(init.body)));
  return { ok: true, status: 200 };
};

// #region example
// Send one trace per run (a span per node, with token cost) to any OTLP/HTTP collector:
// Jaeger, Tempo, Honeycomb, Langfuse, LangSmith, ...
const stop = exportTracesToOtlp(app, {
  endpoint: "http://localhost:4318/v1/traces", // or set AILU_OTEL_EXPORTER_URL
  serviceName: "refund-desk",
  fetchImpl: collector // omit in production: the global fetch is used
});

await app.run();
stop(); // stop exporting
// #endregion example

await new Promise((resolve) => setTimeout(resolve, 50));
assert.ok(sent.length >= 1);
