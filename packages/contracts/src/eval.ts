import { z } from "zod/v3";

export const EvalExampleDtoSchema = z.object({
  id: z.string().min(1),
  input: z.unknown(),
  expectedOutput: z.unknown().optional(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const EvalDatasetDtoSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  description: z.string().optional(),
  examples: z.array(EvalExampleDtoSchema),
  createdAt: z.string().datetime()
});

export const EvalScoreDtoSchema = z.object({
  metric: z.string().min(1),
  value: z.number(),
  weight: z.number().min(0).max(1).optional()
});

export const ExperimentDtoSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  datasetId: z.string().min(1),
  model: z.string().min(1),
  status: z.enum(["queued", "running", "completed", "failed"]),
  createdAt: z.string().datetime(),
  completedAt: z.string().datetime().optional()
});

export const EvalReportDtoSchema = z.object({
  experimentId: z.string().min(1),
  aggregateScore: z.number(),
  scores: z.array(EvalScoreDtoSchema),
  summary: z.string().optional(),
  createdAt: z.string().datetime()
});

export type EvalExampleDto = z.infer<typeof EvalExampleDtoSchema>;
export type EvalDatasetDto = z.infer<typeof EvalDatasetDtoSchema>;
export type ExperimentDto = z.infer<typeof ExperimentDtoSchema>;
export type EvalScoreDto = z.infer<typeof EvalScoreDtoSchema>;
export type EvalReportDto = z.infer<typeof EvalReportDtoSchema>;
