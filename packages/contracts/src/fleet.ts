import { z } from "zod/v3";

export const WorkerStatusSchema = z.enum(["active", "draining", "dead"]);

export const WorkerInfoDtoSchema = z.object({
  id: z.string().min(1),
  hostname: z.string().min(1),
  status: WorkerStatusSchema,
  lastHeartbeatAt: z.string().datetime(),
  currentJobs: z.number().int().min(0)
});

export const QueueDepthDtoSchema = z.object({
  queueName: z.string().min(1),
  depth: z.number().int().min(0),
  delayed: z.number().int().min(0).default(0),
  failed: z.number().int().min(0).default(0)
});

export const FleetSummaryDtoSchema = z.object({
  workers: z.array(WorkerInfoDtoSchema),
  queues: z.array(QueueDepthDtoSchema),
  activeWorkers: z.number().int().min(0),
  drainingWorkers: z.number().int().min(0),
  deadWorkers: z.number().int().min(0)
});

export type WorkerStatus = z.infer<typeof WorkerStatusSchema>;
export type WorkerInfoDto = z.infer<typeof WorkerInfoDtoSchema>;
export type QueueDepthDto = z.infer<typeof QueueDepthDtoSchema>;
export type FleetSummaryDto = z.infer<typeof FleetSummaryDtoSchema>;

export const WorkerRegisterDtoSchema = z.object({
  workerId: z.string().min(1),
  capacity: z.number().int().min(1),
  status: WorkerStatusSchema
});

export const WorkerHeartbeatDtoSchema = z.object({
  workerId: z.string().min(1),
  capacity: z.number().int().min(1),
  activeJobs: z.number().int().min(0),
  status: WorkerStatusSchema
});

export type WorkerRegisterDto = z.infer<typeof WorkerRegisterDtoSchema>;
export type WorkerHeartbeatDto = z.infer<typeof WorkerHeartbeatDtoSchema>;
