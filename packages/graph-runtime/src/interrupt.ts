import type { NodeId } from "@ailu/graph-core";

export type InterruptConfig = {
  before?: NodeId[];
  after?: NodeId[];
};

export class DynamicInterrupt extends Error {
  public readonly reason: string;
  public readonly patch?: Record<string, unknown>;

  public constructor(reason: string, patch?: Record<string, unknown>) {
    super(reason);
    this.name = "DynamicInterrupt";
    this.reason = reason;
    this.patch = patch;
  }
}

export const shouldInterruptBefore = (config: InterruptConfig | undefined, nodeId: NodeId): boolean =>
  config?.before?.includes(nodeId) ?? false;

export const shouldInterruptAfter = (config: InterruptConfig | undefined, nodeId: NodeId): boolean =>
  config?.after?.includes(nodeId) ?? false;
