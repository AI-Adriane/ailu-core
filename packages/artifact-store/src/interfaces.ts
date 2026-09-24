import type { RunId } from "@ailu/graph-core";

import type { Artifact, ArtifactId, ArtifactVersion } from "./types.js";

export interface ArtifactStore {
  write(artifact: Omit<Artifact, "id" | "version" | "createdAt">): Promise<Artifact>;
  read(id: ArtifactId): Promise<Artifact | undefined>;
  readVersion(id: ArtifactId, version: ArtifactVersion): Promise<Artifact | undefined>;
  listByRun(runId: RunId): Promise<Artifact[]>;
  listVersions(id: ArtifactId): Promise<Artifact[]>;
}
