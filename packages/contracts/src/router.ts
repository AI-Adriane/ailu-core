import { z } from "zod/v3";

/** The LLM router's decision for a (knowledge base, capability tier, caller) triple. */
export const RouterDecisionDtoSchema = z.object({
  namespace: z.string().min(1),
  tier: z.string(),
  /** Resolved provider + model (a per-base pin overrides the tier default). */
  provider: z.string(),
  model: z.string(),
  /** True when the model came from the recommended per-tier default (not a pin/override). */
  recommended: z.boolean(),
  /** True when a per-namespace model pin was applied. */
  pinned: z.boolean(),
  /** Caller's effective permissions on this knowledge base. */
  canRead: z.boolean(),
  canWrite: z.boolean()
});

/** Body for pinning a namespace's model/provider (omit a field to leave it tier-resolved). */
export const SetModelPolicyDtoSchema = z.object({
  provider: z.string().min(1).optional(),
  model: z.string().min(1).optional()
});

export type RouterDecisionDto = z.infer<typeof RouterDecisionDtoSchema>;
export type SetModelPolicyDto = z.infer<typeof SetModelPolicyDtoSchema>;
