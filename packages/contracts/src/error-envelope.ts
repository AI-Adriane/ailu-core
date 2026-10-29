import { z } from "zod/v3";

export const ErrorEnvelopeSchema = z.object({
  code: z.string().min(1),
  message: z.string().min(1),
  path: z.string().min(1).optional()
});

export type ErrorEnvelope = z.infer<typeof ErrorEnvelopeSchema>;
