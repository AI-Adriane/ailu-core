import type { GraphState, NodeId } from "@ailu/graph-core";

export const STREAM_MODES = ["values", "updates", "debug", "messages"] as const;
export type StreamMode = (typeof STREAM_MODES)[number];

export type StreamEvent =
  | { type: "state_value"; state: GraphState }
  | { type: "state_update"; delta: Record<string, unknown>; nodeId: NodeId }
  | { type: "message_delta"; delta: string; nodeId: NodeId; messageId: string }
  | { type: "tool_call"; toolId: string; input: unknown; nodeId: NodeId }
  | { type: "debug"; payload: unknown; nodeId: NodeId };
