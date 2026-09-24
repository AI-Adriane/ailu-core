import type { RunId } from "@ailu/graph-core";

import type { Checkpointer } from "./interfaces.js";
import type { Checkpoint, CheckpointId } from "./types.js";

export class InMemoryCheckpointer implements Checkpointer {
  private readonly checkpointsById = new Map<CheckpointId, Checkpoint>();
  private readonly latestCheckpointByRunId = new Map<RunId, CheckpointId>();
  private readonly checkpointIdsByRunId = new Map<RunId, CheckpointId[]>();

  public async save(checkpoint: Checkpoint): Promise<void> {
    this.checkpointsById.set(checkpoint.id, checkpoint);
    this.latestCheckpointByRunId.set(checkpoint.runId, checkpoint.id);
    const ids = this.checkpointIdsByRunId.get(checkpoint.runId) ?? [];
    this.checkpointIdsByRunId.set(checkpoint.runId, [...ids, checkpoint.id]);
  }

  public async load(runId: RunId): Promise<Checkpoint | undefined> {
    const checkpointId = this.latestCheckpointByRunId.get(runId);
    if (checkpointId === undefined) {
      return undefined;
    }

    return this.checkpointsById.get(checkpointId);
  }

  public async loadById(id: CheckpointId): Promise<Checkpoint | undefined> {
    return this.checkpointsById.get(id);
  }

  public async list(runId: RunId): Promise<Checkpoint[]> {
    const ids = this.checkpointIdsByRunId.get(runId) ?? [];
    return ids
      .map((id) => this.checkpointsById.get(id))
      .filter((checkpoint): checkpoint is Checkpoint => checkpoint !== undefined);
  }
}
