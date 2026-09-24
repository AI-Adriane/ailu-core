import type { NodeId } from "@ailu/graph-core";

import type { NodeHandler, NodeRegistry } from "./interfaces.js";

export class InMemoryNodeRegistry implements NodeRegistry {
  private readonly handlers = new Map<NodeId, NodeHandler>();

  public register(nodeId: NodeId, handler: NodeHandler): void {
    this.handlers.set(nodeId, handler);
  }

  public resolve(nodeId: NodeId): NodeHandler | undefined {
    return this.handlers.get(nodeId);
  }
}
