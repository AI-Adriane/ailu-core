import { z } from "zod/v3";

export const ArtifactDtoSchema = z.object({
  runId: z.string().min(1),
  name: z.string().min(1),
  contentType: z.string().min(1),
  content: z.unknown(),
  createdAt: z.string().datetime()
});

export type ArtifactDto = z.infer<typeof ArtifactDtoSchema>;
