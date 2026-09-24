import type { EdgeDefinition, GraphDefinition, NodeDefinition } from "@ailu/graph-core";

type MessageGraphChannels = {
  messages: {
    type: "messages";
    reducer: "append";
    default: [];
  };
};

export type MessageGraph = GraphDefinition<MessageGraphChannels>;

export const createMessageGraph = (
  nodes: NodeDefinition[],
  edges: EdgeDefinition[]
): MessageGraph => {
  const entryNode = nodes[0];
  if (entryNode === undefined) {
    throw new Error("createMessageGraph requires at least one node.");
  }
  return {
    id: `message-graph-${Date.now()}` as GraphDefinition["id"],
    version: "1.0.0",
    name: "MessageGraph",
    channels: {
      messages: {
        type: "messages",
        reducer: "append",
        default: []
      }
    },
    nodes,
    edges,
    entryNodeId: entryNode.id
  };
};
