import type { RunExplanation } from "@ailu-ai/graph-sdk";

export const MAX_SUSPENDED_RUNS = 128;
export const MAX_EXPLANATION_BYTES = 64 * 1024;
export const MAX_RETAINED_BYTES = 1024 * 1024;
export const SUSPENDED_RUN_TTL_MS = 15 * 60 * 1000;

type Snapshot = {
  json: string;
  bytes: number;
  expiresAt: number;
  timer: ReturnType<typeof setTimeout>;
};

/** Read-only, detached explanations. Never retains a graph, gateway or channel payload.
 * FIFO eviction bounds count and UTF-8 bytes; fixed expiry also releases idle entries.
 * Reads do not extend the lifetime of a snapshot. */
export class SuspendedRunSnapshots {
  private readonly entries = new Map<string, Snapshot>();
  private retainedBytes = 0;

  public set(runId: string, explanation: RunExplanation): boolean {
    const json = JSON.stringify(explanation);
    const bytes = Buffer.byteLength(json, "utf8");
    this.delete(runId);
    // Channel names can be caller-controlled. Do not truncate or retain oversized data.
    if (bytes > MAX_EXPLANATION_BYTES) return false;

    while (
      this.entries.size >= MAX_SUSPENDED_RUNS ||
      this.retainedBytes + bytes > MAX_RETAINED_BYTES
    ) {
      this.delete(this.entries.keys().next().value!);
    }
    const timer = setTimeout(() => this.delete(runId), SUSPENDED_RUN_TTL_MS);
    timer.unref();
    this.entries.set(runId, {
      json,
      bytes,
      expiresAt: Date.now() + SUSPENDED_RUN_TTL_MS,
      timer
    });
    this.retainedBytes += bytes;
    return true;
  }

  public get(runId: string): RunExplanation | undefined {
    const entry = this.entries.get(runId);
    if (entry === undefined) return undefined;
    if (Date.now() >= entry.expiresAt) {
      this.delete(runId);
      return undefined;
    }
    return JSON.parse(entry.json) as RunExplanation;
  }

  private delete(runId: string): void {
    const entry = this.entries.get(runId);
    if (entry === undefined) return;
    clearTimeout(entry.timer);
    this.retainedBytes -= entry.bytes;
    this.entries.delete(runId);
  }
}
