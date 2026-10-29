import { z } from "zod/v3";

export const SpanDtoSchema = z.object({
  id: z.string().min(1),
  traceId: z.string().min(1),
  parentSpanId: z.string().min(1).optional(),
  name: z.string().min(1),
  runId: z.string().min(1).optional(),
  nodeId: z.string().min(1).optional(),
  startedAt: z.string().datetime(),
  endedAt: z.string().datetime().optional(),
  status: z.enum(["ok", "error"]),
  attributes: z.record(z.string(), z.unknown()),
  error: z.string().optional()
});

export const TraceDtoSchema = z.object({
  traceId: z.string().min(1),
  spans: z.array(SpanDtoSchema)
});

export const MetricDtoSchema = z.object({
  name: z.string().min(1),
  value: z.number(),
  unit: z.string().min(1),
  tags: z.record(z.string(), z.string()),
  timestamp: z.string().datetime()
});

export const AlertRuleDtoSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  metric: z.string().min(1),
  operator: z.enum(["gt", "gte", "lt", "lte", "eq"]),
  threshold: z.number(),
  enabled: z.boolean()
});

export type TraceDto = z.infer<typeof TraceDtoSchema>;
export type SpanDto = z.infer<typeof SpanDtoSchema>;
export type MetricDto = z.infer<typeof MetricDtoSchema>;
export type AlertRuleDto = z.infer<typeof AlertRuleDtoSchema>;
