import { z } from "zod";

// "rejected" (product ADR 0068, issue #496): a governance-level terminal status the
// control plane assigns when a human REJECTS a pending `human-gate` approval — distinct
// from "suspended" (still awaiting a decision) and from "failed" (an execution error).
// The engine itself has no notion of rejection; a rejected run's underlying GraphState
// stays "suspended" forever (the engine's own resume() would otherwise unconditionally
// ADVANCE past a human-gate node) — the control plane refuses to call resume() at all
// once it observes a rejected gate approval, and surfaces "rejected" on the DTO instead.
// "cancelled" (ADR 0044): a run STOPPED on purpose at a node boundary — a human kill switch or
// a circuit breaker asked for it, the engine finished the node it was on, checkpointed, and went
// terminal. Distinct from "failed" (nothing malfunctioned) and from "rejected" (no gate was
// refused — there may not even be a gate). Unlike "rejected", this one IS an engine status: the
// underlying GraphState really is `cancelled`, so the run stays resumable/replayable from its
// last checkpoint and the control plane surfaces exactly what the engine recorded.
const GraphStatusSchema = z.enum([
  "idle",
  "running",
  "suspended",
  "completed",
  "failed",
  "rejected",
  "cancelled"
]);

export const CreateRunDtoSchema = z.object({
  graphId: z.string().min(1),
  initialData: z.record(z.string(), z.unknown()).default({})
});

export const RunDtoSchema = z.object({
  id: z.string().min(1),
  graphId: z.string().min(1),
  status: GraphStatusSchema,
  currentNodeId: z.string().min(1).optional(),
  channels: z.record(z.string(), z.unknown()),
  version: z.number().int().min(0),
  checkpointId: z.string().min(1).optional(),
  /** Owning tenant (tenancy). Optional for pre-tenancy rows backfilled to `default`. */
  tenantId: z.string().min(1).optional(),
  /** Id of the principal who started the run. */
  createdBy: z.string().min(1).optional(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime()
});

/** Lightweight row for the runs list — no channels, cheap to query at scale. */
export const RunSummaryDtoSchema = z.object({
  id: z.string().min(1),
  graphId: z.string().min(1),
  status: GraphStatusSchema,
  currentNodeId: z.string().min(1).nullable(),
  tenantId: z.string().min(1).optional(),
  createdBy: z.string().min(1).optional(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime()
});

export const RunListDtoSchema = z.object({
  items: z.array(RunSummaryDtoSchema),
  total: z.number().int().min(0),
  limit: z.number().int().min(1),
  offset: z.number().int().min(0)
});

/** Query params arrive as strings over HTTP — coerce numerics. */
export const ListRunsQueryDtoSchema = z.object({
  status: GraphStatusSchema.optional(),
  graphId: z.string().min(1).optional(),
  limit: z.coerce.number().int().min(1).max(200).default(50),
  offset: z.coerce.number().int().min(0).default(0)
});

export const InterruptRunDtoSchema = z.object({
  nodeId: z.string().min(1),
  when: z.enum(["before", "after"])
});

/**
 * Channel keys the runtime governance machinery owns: `__approvedTools` carries the
 * human-granted tools an agent may run past an approval gate, and `__approvalIds`
 * carries the pending ApprovalEngine request ids. Allowing a client to PATCH these
 * directly would be a self-approval back door (forge an unlock, or erase the pending
 * ids that the resume gate checks), so they are rejected by the patch DTO below. The
 * literals mirror `@ailu/graph-sdk`'s `APPROVED_TOOLS_CHANNEL` / `APPROVAL_IDS_CHANNEL`;
 * contracts stays dependency-free of the SDK, so they are duplicated as constants here.
 */
export const RESERVED_PATCH_CHANNELS = ["__approvedTools", "__approvalIds"] as const;

export const PatchRunStateDtoSchema = z.object({
  patch: z
    .record(z.string(), z.unknown())
    .refine(
      (patch) => RESERVED_PATCH_CHANNELS.every((channel) => !(channel in patch)),
      (patch) => ({
        message: `patch may not write the reserved governance channel(s): ${RESERVED_PATCH_CHANNELS.filter(
          (channel) => channel in patch
        ).join(", ")}`
      })
    ),
  resumeFrom: z.string().min(1).optional()
});

export const ReplayRunDtoSchema = z.object({
  checkpointId: z.string().min(1)
});

export const CheckpointDtoSchema = z.object({
  id: z.string().min(1),
  runId: z.string().min(1),
  graphState: z.object({
    runId: z.string().min(1),
    graphId: z.string().min(1),
    currentNodeId: z.string().min(1),
    status: GraphStatusSchema,
    channels: z.record(z.string(), z.unknown()),
    version: z.number().int().min(0),
    checkpointId: z.string().min(1).optional(),
    createdAt: z.string().datetime(),
    updatedAt: z.string().datetime()
  }),
  createdAt: z.string().datetime()
});

export type CreateRunDto = z.infer<typeof CreateRunDtoSchema>;
export type RunDto = z.infer<typeof RunDtoSchema>;
export type RunSummaryDto = z.infer<typeof RunSummaryDtoSchema>;
export type RunListDto = z.infer<typeof RunListDtoSchema>;
export type ListRunsQueryDto = z.infer<typeof ListRunsQueryDtoSchema>;
export type InterruptRunDto = z.infer<typeof InterruptRunDtoSchema>;
export type PatchRunStateDto = z.infer<typeof PatchRunStateDtoSchema>;
export type ReplayRunDto = z.infer<typeof ReplayRunDtoSchema>;
export type CheckpointDto = z.infer<typeof CheckpointDtoSchema>;
