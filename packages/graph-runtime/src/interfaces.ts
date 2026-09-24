import type {
  ChannelsSchema,
  Command,
  GraphState,
  NodeId,
  ResolvedChannels,
  RunId
} from "@ailu/graph-core";
import type { BaseStore } from "../../memory-store/src/interfaces.js";

import type { Checkpoint, CheckpointId, RunEvent } from "./types.js";

export type NodeExecutionContext = {
  memory: BaseStore;
};

export type NodeHandler<
  TChannels extends ChannelsSchema = ChannelsSchema,
  TInput = unknown,
  TOutput extends Partial<ResolvedChannels<TChannels>> | Command<TChannels> =
    | Partial<ResolvedChannels<TChannels>>
    | Command<TChannels>
> = (
  input: TInput,
  state: GraphState<TChannels>,
  context: NodeExecutionContext
) => Promise<TOutput>;

export interface NodeRegistry {
  register(nodeId: NodeId, handler: NodeHandler): void;
  resolve(nodeId: NodeId): NodeHandler | undefined;
}

export type ConditionFn = (state: GraphState) => boolean;

export interface ConditionRegistry {
  register(name: string, fn: ConditionFn): void;
  resolve(name: string): ConditionFn | undefined;
}

export interface Checkpointer {
  save(checkpoint: Checkpoint): Promise<void>;
  load(runId: RunId): Promise<Checkpoint | undefined>;
  loadById(id: CheckpointId): Promise<Checkpoint | undefined>;
  list(runId: RunId): Promise<Checkpoint[]>;
}

export interface EventBus {
  emit(event: RunEvent): void;
  subscribe(handler: (event: RunEvent) => void): () => void;
}
