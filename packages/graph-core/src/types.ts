import type { Message } from "./messages";

export type Brand<T, TBrand extends string> = T & { readonly __brand: TBrand };

export type NodeId = Brand<string, "NodeId">;
export type EdgeId = Brand<string, "EdgeId">;
export type GraphId = Brand<string, "GraphId">;
export type RunId = Brand<string, "RunId">;
export type ChannelReducer = "replace" | "append" | "merge";

export type ChannelDefinition<T> = {
  type: string;
  reducer: ChannelReducer;
  default?: T;
  /** ADR 0032: never emit this channel's value in run events/logs (masked; still checkpointed). */
  noLog?: boolean;
};

export type MessagesChannelDefinition = {
  type: "messages";
  reducer: "append";
  default?: Message[];
};

export type ChannelsSchema = Record<string, ChannelDefinition<unknown>>;

export type ResolvedChannels<TChannels extends ChannelsSchema> = {
  [K in keyof TChannels]: TChannels[K] extends ChannelDefinition<infer TValue> ? TValue : never;
};

export const NODE_TYPES = ["action", "agent", "tool", "human-gate", "subgraph"] as const;
export type NodeType = (typeof NODE_TYPES)[number];

/** ADR 0076 (product repo) — `"error"` routes from a node to a designated handler once its
 * `retryPolicy` is exhausted, instead of failing the whole run. Absent (the common case): failure
 * behavior is unchanged — the run terminates exactly as before this type existed. */
export const EDGE_TYPES = ["default", "conditional", "error"] as const;
export type EdgeType = (typeof EDGE_TYPES)[number];

/** ADR 0076 — coarse classification of why a node's handler threw, attached to `node_failed`/
 * `node_error_routed` events so a governed error path can react differently to a transient
 * failure (worth a human-visible retry) vs a permanent one (worth surfacing, not retrying).
 * Derived by `graph-runtime` from a thrown error's own `failureCategory` property when present
 * (a convention, not an interface — any thrower, e.g. `llm-gateway`, can set it); defaults to
 * `"unknown"` for a plain `Error`/thrown value that doesn't declare one. */
export const FAILURE_CATEGORIES = ["transient", "permanent", "unknown"] as const;
export type FailureCategory = (typeof FAILURE_CATEGORIES)[number];

/** `cancelled` (ADR 0044): terminal, reached when the embedder's cancellation seam returned
 *  true at a node BOUNDARY — the run stopped before scheduling its next node, with its last
 *  checkpoint durable and authoritative. Distinct from `failed` (no error occurred) and from
 *  `suspended` (it is not waiting on a decision). A cancelled run stays replayable. */
export const GRAPH_STATUSES = [
  "idle",
  "running",
  "suspended",
  "completed",
  "failed",
  "cancelled"
] as const;
export type GraphStatus = (typeof GRAPH_STATUSES)[number];

export type RetryPolicy = {
  maxAttempts: number;
  backoffMs: number;
};

export type Command<TChannels extends ChannelsSchema = ChannelsSchema> = {
  goto: NodeId | NodeId[];
  update?: Partial<ResolvedChannels<TChannels>>;
};

export type NodeDefinition = {
  id: NodeId;
  type: NodeType;
  label: string;
  subgraphId?: GraphId;
  inputMapping?: Record<string, string>;
  outputMapping?: Record<string, string>;
  fanOut?: { parallelTo: NodeId[]; joinAt: NodeId };
  /** ADR 0042 D2/D3 (product ADR 0068 — child workflows): dynamic N-child subgraph fan-out.
   * Present on a `type: "subgraph"` node ALONGSIDE its `subgraphId` (never duplicated here) —
   * `overChannel` is read as a JSON array at execution time, one child spawn per item, run
   * concurrently; results land in `joinAt` as a JSON array, input order preserved. Absent (the
   * common case): the node runs its subgraph exactly once, unchanged. */
  mapSubgraph?: { overChannel: string; joinAt: string };
  retryPolicy?: RetryPolicy;
  metadata?: Record<string, unknown>;
};

export type EdgeDefinition = {
  id: EdgeId;
  from: NodeId;
  to: NodeId;
  type: EdgeType;
  condition?: string;
};

export type GraphState<TChannels extends ChannelsSchema = ChannelsSchema> = {
  runId: RunId;
  graphId: GraphId;
  currentNodeId: NodeId;
  status: GraphStatus;
  channels: ResolvedChannels<TChannels>;
  version: number;
  checkpointId?: string;
  createdAt: string;
  updatedAt: string;
};

export type GraphDefinition<TChannels extends ChannelsSchema = ChannelsSchema> = {
  id: GraphId;
  version: string;
  name: string;
  recursionLimit?: number;
  channels: TChannels;
  nodes: NodeDefinition[];
  edges: EdgeDefinition[];
  entryNodeId: NodeId;
  metadata?: Record<string, unknown>;
};

export const trimMessages = (
  messages: Message[],
  maxTokens: number,
  countFn: (message: Message) => number
): Message[] => {
  if (maxTokens <= 0) {
    return [];
  }
  const reversed: Message[] = [];
  let total = 0;
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const message = messages[i];
    if (message === undefined) {
      continue;
    }
    const cost = countFn(message);
    if (total + cost > maxTokens) {
      continue;
    }
    total += cost;
    reversed.push(message);
  }
  return reversed.reverse();
};

export const filterMessages = (messages: Message[], roles: Message["role"][]): Message[] => {
  const allowed = new Set(roles);
  return messages.filter((message) => allowed.has(message.role));
};
