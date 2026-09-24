import type { NodeId, RunId } from "@ailu/graph-core";
import type { RunEvent } from "@ailu/graph-runtime";

export type TraceId = string & { readonly __brand: "TraceId" };
export type SpanId = string & { readonly __brand: "SpanId" };

export const SPAN_STATUSES = ["ok", "error"] as const;
export type SpanStatus = (typeof SPAN_STATUSES)[number];

export type Span = {
  id: SpanId;
  traceId: TraceId;
  parentSpanId?: SpanId;
  name: string;
  runId?: RunId;
  nodeId?: NodeId;
  startedAt: Date;
  endedAt?: Date;
  status: SpanStatus;
  attributes: Record<string, unknown>;
  error?: string;
};

export type Metric = {
  name: string;
  value: number;
  unit: string;
  tags: Record<string, string>;
  timestamp: Date;
};

export type ObservabilityEvent = RunEvent | Span | Metric;
