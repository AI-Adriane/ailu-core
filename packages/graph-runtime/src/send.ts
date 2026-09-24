import type { NodeId, RunId } from "@ailu/graph-core";

export type SendEnvelope = {
  runId: RunId;
  nodeId: NodeId;
  input: unknown;
};
