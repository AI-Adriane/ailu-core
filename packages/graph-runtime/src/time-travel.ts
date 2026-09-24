import type { RunId } from "@ailu-ai/graph-core";

export const createForkRunId = (runId: RunId): RunId =>
  `${String(runId)}:fork:${Date.now()}:${Math.random().toString(36).slice(2, 8)}` as RunId;
