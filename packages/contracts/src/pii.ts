import { z } from "zod/v3";

/**
 * PII redaction DTOs (ADR 0008). A per-namespace policy decides whether personal data is
 * detected, redacted before reaching a model, or blocks the operation. Mirrors the router
 * policy shape.
 */
export const PiiLevelSchema = z.enum(["off", "detect", "redact", "block"]);
export type PiiLevel = z.infer<typeof PiiLevelSchema>;

/** Body for PUT /pii/namespace/:ns/policy (owner-only). */
export const SetPiiPolicyDtoSchema = z.object({
  level: PiiLevelSchema.optional(),
  entities: z.array(z.string()).optional(),
  threshold: z.number().min(0).max(1).optional()
});
export type SetPiiPolicyDto = z.infer<typeof SetPiiPolicyDtoSchema>;

/** The resolved policy for a namespace. */
export const PiiPolicyDtoSchema = z.object({
  namespace: z.string(),
  level: PiiLevelSchema,
  entities: z.array(z.string()),
  threshold: z.number(),
  canWrite: z.boolean()
});
export type PiiPolicyDto = z.infer<typeof PiiPolicyDtoSchema>;

/** A single detection (entity type, confidence, span). */
export const PiiFindingDtoSchema = z.object({
  entity: z.string(),
  score: z.number(),
  start: z.number().int(),
  end: z.number().int()
});
export type PiiFindingDto = z.infer<typeof PiiFindingDtoSchema>;

/**
 * Wire contract for the native gateway seam (ADR 0008 phase 2). The Rust engine POSTs the
 * outbound LLM texts (system + messages) to the control plane, which returns them redacted in
 * the same order. Same length in, same length out.
 */
export const PiiRedactBatchDtoSchema = z.object({
  texts: z.array(z.string())
});
export type PiiRedactBatchDto = z.infer<typeof PiiRedactBatchDtoSchema>;

/** `blocked` is true when a `block`-level policy matched — the engine then fails the call. */
export const PiiRedactBatchResultDtoSchema = z.object({
  texts: z.array(z.string()),
  blocked: z.boolean()
});
export type PiiRedactBatchResultDto = z.infer<typeof PiiRedactBatchResultDtoSchema>;
