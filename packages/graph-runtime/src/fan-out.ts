import type { NodeId } from "@ailu-ai/graph-core";

export type FanOutPlan = {
  parallelTo: NodeId[];
  joinAt: NodeId;
};
