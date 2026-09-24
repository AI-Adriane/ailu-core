import type { NodeId, RunId } from "@ailu-ai/graph-core";

export type ArtifactId = string & { readonly __brand: "ArtifactId" };
export type ArtifactVersion = number;

export const ARTIFACT_MEDIA_TYPES = [
  "application/json",
  "text/plain",
  "text/markdown",
  "application/octet-stream"
] as const;
export type ArtifactMediaType = (typeof ARTIFACT_MEDIA_TYPES)[number];

export type Artifact = {
  id: ArtifactId;
  runId: RunId;
  nodeId: NodeId;
  name: string;
  mediaType: ArtifactMediaType;
  version: ArtifactVersion;
  content: unknown;
  createdAt: Date;
  metadata?: Record<string, unknown>;
};

export type ArtifactRef = {
  id: ArtifactId;
  version: ArtifactVersion;
};
