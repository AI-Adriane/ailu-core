import { z } from "zod/v3";

/**
 * The governed virtual filesystem per-path permission policy (ADR 0024 phase 2d) —
 * the API↔Studio boundary for managing fs access, modeled on the per-namespace PII
 * policy (see `pii.ts`). Rules are owner-only; the control plane compiles a run's rules
 * into the engine's `EngineSpec.fsPolicy` (fail-closed: an unmatched path → `read`).
 */

/** A filesystem permission verb: `deny` < `read` < `gate` < `write`. */
export const FsPermVerbSchema = z.enum(["deny", "read", "write", "gate"]);
export type FsPermVerb = z.infer<typeof FsPermVerbSchema>;

/** One per-path rule: a glob (`*` within a segment, `**` across) mapped to a verb. */
export const FsPathRuleSchema = z.object({
  glob: z.string().min(1),
  verb: FsPermVerbSchema
});
export type FsPathRule = z.infer<typeof FsPathRuleSchema>;

/** Owner-only PUT body: replace a namespace's fs path rules. */
export const SetFsPolicyDtoSchema = z.object({
  rules: z.array(FsPathRuleSchema)
});
export type SetFsPolicyDto = z.infer<typeof SetFsPolicyDtoSchema>;

/** The resolved policy for a namespace, with server-computed capability flags. */
export const FsPolicyDtoSchema = z.object({
  namespace: z.string(),
  rules: z.array(FsPathRuleSchema),
  /** `true` when the caller's tenant role may edit this policy (owner-only). */
  canEditPolicy: z.boolean()
});
export type FsPolicyDto = z.infer<typeof FsPolicyDtoSchema>;

/** Resolve a single path against a namespace's policy (for the editor / a preview). */
export const FsResolvedPolicyDtoSchema = z.object({
  path: z.string(),
  verb: FsPermVerbSchema,
  canRead: z.boolean(),
  /** `true` when `verb` is `write` or `gate`. */
  canWrite: z.boolean(),
  /** `true` when `verb` is `gate` (a write routes through an approval). */
  requiresGate: z.boolean()
});
export type FsResolvedPolicyDto = z.infer<typeof FsResolvedPolicyDtoSchema>;
