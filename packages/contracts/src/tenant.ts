import { z } from "zod/v3";

import { TenantRoleSchema } from "./principal.js";

export const TenantQuotaDtoSchema = z.object({
  /** Max agents (graphs/workflows) the tenant may create on its plan. */
  maxAgents: z.number().int().min(0),
  maxRunsPerDay: z.number().int().min(0),
  maxArtifactsStorageMb: z.number().int().min(0),
  maxConcurrentRuns: z.number().int().min(0)
});

export const TenantUsageDtoSchema = z.object({
  /** Agents (graphs) currently owned by the tenant — computed live. */
  activeAgents: z.number().int().min(0),
  runsToday: z.number().int().min(0),
  artifactsStorageMb: z.number().min(0),
  concurrentRuns: z.number().int().min(0),
  updatedAt: z.string().datetime()
});

export const TenantDtoSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  plan: z.enum(["free", "pro", "enterprise"]),
  quota: TenantQuotaDtoSchema,
  usage: TenantUsageDtoSchema
});

export type TenantDto = z.infer<typeof TenantDtoSchema>;
export type TenantQuotaDto = z.infer<typeof TenantQuotaDtoSchema>;
export type TenantUsageDto = z.infer<typeof TenantUsageDtoSchema>;

// ── Team / membership management ──────────────────────────────────────────────────────
export const MemberDtoSchema = z.object({
  userId: z.string(),
  email: z.string().email(),
  displayName: z.string().nullable(),
  role: TenantRoleSchema,
  grantedAt: z.string()
});
export type MemberDto = z.infer<typeof MemberDtoSchema>;

export const InviteMemberDtoSchema = z.object({
  email: z.string().email(),
  displayName: z.string().min(1).optional(),
  role: TenantRoleSchema
});
export type InviteMemberDto = z.infer<typeof InviteMemberDtoSchema>;

export const UpdateMemberRoleDtoSchema = z.object({ role: TenantRoleSchema });
export type UpdateMemberRoleDto = z.infer<typeof UpdateMemberRoleDtoSchema>;

/** Invite result. The accept-invite email is sent automatically; `inviteLink` is returned
 * ONLY when email delivery is not configured (dev), so the owner can share the link manually.
 * Null when the invitation was emailed (or when an existing user was simply added). */
export const InviteResultDtoSchema = z.object({
  member: MemberDtoSchema,
  inviteLink: z.string().nullable()
});
export type InviteResultDto = z.infer<typeof InviteResultDtoSchema>;
