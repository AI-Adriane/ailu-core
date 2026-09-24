import type { Command, GraphState } from "@ailu-ai/graph-core";
import type { NodeExecutionContext } from "@ailu-ai/graph-runtime";

/** A map of channel name → value type. The generic that flows through the builder. */
export type ChannelValues = Record<string, unknown>;

/** The starting (channel-less) state of a fresh {@link import("./builder.js").GraphBuilder}. */
export type EmptyChannels = Record<never, never>;

/** {@link GraphState} with strongly-typed channels. */
export type TypedGraphState<TState extends ChannelValues> = Omit<GraphState, "channels"> & {
  channels: TState;
};

/**
 * What a node handler may return: a partial update to the declared channels
 * (type-checked) plus any additional keys (the runtime passes unknown keys
 * through), or a routing {@link Command}.
 */
export type ChannelUpdate<TState extends ChannelValues> =
  | (Partial<TState> & Record<string, unknown>)
  | Command;

/** A node handler with strongly-typed state and return value. */
export type TypedNodeHandler<TState extends ChannelValues> = (
  input: unknown,
  state: TypedGraphState<TState>,
  context: NodeExecutionContext
) => Promise<ChannelUpdate<TState>>;

/** A conditional-edge predicate over strongly-typed state. */
export type TypedCondition<TState extends ChannelValues> = (state: TypedGraphState<TState>) => boolean;

/** Initial data accepted by a run: declared channels (typed) plus arbitrary extras. */
export type InitialData<TState extends ChannelValues> = Partial<TState> & Record<string, unknown>;
