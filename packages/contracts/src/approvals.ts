import { z } from "zod/v3";

const ApprovalStatusSchema = z.enum(["pending", "approved", "rejected"]);

export const ApprovalDtoSchema = z.object({
  id: z.string().min(1),
  runId: z.string().min(1),
  nodeId: z.string().min(1),
  requestedBy: z.string().min(1),
  /** Human-readable subject (e.g. `tool:refund`). */
  subject: z.string().min(1),
  status: ApprovalStatusSchema,
  resolvedBy: z.string().min(1).optional(),
  reason: z.string().min(1).optional(),
  /** Owning tenant (tenancy). Optional for pre-tenancy rows backfilled to `default`. */
  tenantId: z.string().min(1).optional(),
  requestedAt: z.string().datetime(),
  decidedAt: z.string().datetime().optional()
});

export type ApprovalDto = z.infer<typeof ApprovalDtoSchema>;

/**
 * BREAKING (tenancy/auth): `resolvedBy` is no longer accepted in the request body. The
 * server derives the resolver from the authenticated principal (`@CurrentUser().id`) so
 * a caller can NOT forge who approved/rejected a gate (which would also defeat the
 * no-self-approval invariant). Approve carries no body; reject carries only `reason`.
 */
export const ApproveApprovalDtoSchema = z.object({}).strict();

export const RejectApprovalDtoSchema = z.object({
  reason: z.string().min(1)
});

export type ApproveApprovalDto = z.infer<typeof ApproveApprovalDtoSchema>;
export type RejectApprovalDto = z.infer<typeof RejectApprovalDtoSchema>;
