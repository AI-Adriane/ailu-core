import type { NodeId } from "@ailu/graph-core";

export type FanOutPlan = {
  parallelTo: NodeId[];
  joinAt: NodeId;
};
