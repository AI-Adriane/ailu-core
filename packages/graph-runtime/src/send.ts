import type { NodeId, RunId } from "@ailu-ai/graph-core";

export type SendEnvelope = {
  runId: RunId;
  nodeId: NodeId;
  input: unknown;
};
