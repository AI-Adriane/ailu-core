import type { RunId } from "@ailu-ai/graph-core";

import type { ArtifactStore } from "./interfaces.js";
import type { Artifact, ArtifactId, ArtifactVersion } from "./types.js";

type ArtifactWriteInput = Omit<Artifact, "id" | "version" | "createdAt">;
type ArtifactKey = `${string}:${string}`;

const makeKey = (runId: RunId, name: string): ArtifactKey => `${String(runId)}:${name}`;
const createArtifactId = (runId: RunId, name: string): ArtifactId =>
  `${String(runId)}:${name}` as ArtifactId;

export class InMemoryArtifactStore implements ArtifactStore {
  private readonly artifactsById = new Map<ArtifactId, Artifact[]>();
  private readonly latestIdByRunAndName = new Map<ArtifactKey, ArtifactId>();

  public async write(artifact: ArtifactWriteInput): Promise<Artifact> {
    const key = makeKey(artifact.runId, artifact.name);
    const artifactId = this.latestIdByRunAndName.get(key) ?? createArtifactId(artifact.runId, artifact.name);
    const versions = this.artifactsById.get(artifactId) ?? [];
    const nextVersion = (versions.at(-1)?.version ?? 0) + 1;

    const nextArtifact: Artifact = {
      ...artifact,
      id: artifactId,
      version: nextVersion,
      createdAt: new Date()
    };

    versions.push(nextArtifact);
    this.artifactsById.set(artifactId, versions);
    this.latestIdByRunAndName.set(key, artifactId);

    return nextArtifact;
  }

  public async read(id: ArtifactId): Promise<Artifact | undefined> {
    return this.artifactsById.get(id)?.at(-1);
  }

  public async readVersion(
    id: ArtifactId,
    version: ArtifactVersion
  ): Promise<Artifact | undefined> {
    return this.artifactsById.get(id)?.find((artifact) => artifact.version === version);
  }

  public async listByRun(runId: RunId): Promise<Artifact[]> {
    const runArtifacts: Artifact[] = [];
    for (const versions of this.artifactsById.values()) {
      for (const artifact of versions) {
        if (artifact.runId === runId) {
          runArtifacts.push(artifact);
        }
      }
    }

    return runArtifacts;
  }

  public async listVersions(id: ArtifactId): Promise<Artifact[]> {
    return [...(this.artifactsById.get(id) ?? [])];
  }
}
