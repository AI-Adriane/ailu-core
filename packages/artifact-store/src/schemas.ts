import { z } from "zod/v3";

import { ARTIFACT_MEDIA_TYPES } from "./types.js";

export const ArtifactMediaTypeSchema = z.enum(ARTIFACT_MEDIA_TYPES);

export const ArtifactSchema = z.object({
  id: z.string().min(1).brand<"ArtifactId">(),
  runId: z.string().min(1).brand<"RunId">(),
  nodeId: z.string().min(1).brand<"NodeId">(),
  name: z.string().min(1),
  mediaType: ArtifactMediaTypeSchema,
  version: z.number().int().min(1),
  content: z.unknown(),
  createdAt: z.date(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const ArtifactRefSchema = z.object({
  id: z.string().min(1).brand<"ArtifactId">(),
  version: z.number().int().min(1)
});
