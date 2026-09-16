import { z } from "zod";

const BaseEventSchema = z.object({
  type: z.string().min(1),
  runId: z.string().min(1),
  timestamp: z.string().datetime()
});

const NodeStartedSchema = BaseEventSchema.extend({
  type: z.literal("node_started"),
  nodeId: z.string().min(1)
});

const NodeCompletedSchema = BaseEventSchema.extend({
  type: z.literal("node_completed"),
  nodeId: z.string().min(1),
  output: z.unknown()
});

const NodeFailedSchema = BaseEventSchema.extend({
  type: z.literal("node_failed"),
  nodeId: z.string().min(1),
  error: z.string().min(1),
  attempt: z.number().int().min(1)
});

const RunSuspendedSchema = BaseEventSchema.extend({
  type: z.literal("run_suspended"),
  nodeId: z.string().min(1),
  reason: z.string().min(1)
});

const RunResumedSchema = BaseEventSchema.extend({
  type: z.literal("run_resumed"),
  nodeId: z.string().min(1)
});

const RunCompletedSchema = BaseEventSchema.extend({
  type: z.literal("run_completed"),
  finalState: z.object({
    runId: z.string().min(1),
    graphId: z.string().min(1),
    currentNodeId: z.string().min(1),
    status: z.enum(["idle", "running", "suspended", "completed", "failed", "cancelled"]),
    channels: z.record(z.string(), z.unknown()),
    version: z.number().int().min(0),
    checkpointId: z.string().min(1).optional(),
    createdAt: z.string().datetime(),
    updatedAt: z.string().datetime()
  })
});

const RunFailedSchema = BaseEventSchema.extend({
  type: z.literal("run_failed"),
  error: z.string().min(1)
});

// ADR 0044: terminal cooperative cancellation at a node BOUNDARY. `nodeId` is the node the run
// was ABOUT to execute when the cancellation seam was observed — it never ran. Emitted once,
// after the final checkpoint is durable; never alongside `run_completed` / `run_failed`.
const RunCancelledSchema = BaseEventSchema.extend({
  type: z.literal("run_cancelled"),
  nodeId: z.string().min(1)
});

// ADR 0033 phase 13: an observational per-token delta. Does NOT extend BaseEventSchema
// because its `timestamp` is the engine's millis-since-epoch string (the same value every
// Rust event carries on the napi wire), not the ISO datetime the control plane re-stamps on
// the persisted/API path — see ADR 0033 resolution #3 (the pre-existing Rust↔TS timestamp
// divergence, out of scope here). `parentRunId`/`spawnId` tag a `mapAgents` sub-agent stream.
const TokenDeltaSchema = z.object({
  type: z.literal("token_delta"),
  runId: z.string().min(1),
  nodeId: z.string().min(1),
  messageId: z.string().min(1),
  delta: z.string(),
  parentRunId: z.string().min(1).optional(),
  spawnId: z.number().int().min(0).optional(),
  timestamp: z.string().min(1)
});

export const RunEventDtoSchema = z.discriminatedUnion("type", [
  NodeStartedSchema,
  NodeCompletedSchema,
  NodeFailedSchema,
  RunSuspendedSchema,
  RunResumedSchema,
  RunCompletedSchema,
  RunFailedSchema,
  RunCancelledSchema,
  TokenDeltaSchema
]);

export type RunEventDto = z.infer<typeof RunEventDtoSchema>;

/** A run lifecycle event as persisted in the control plane's event journal. */
export const PersistedRunEventDtoSchema = z.object({
  id: z.string().min(1),
  runId: z.string().min(1),
  type: z.string().min(1),
  payload: z.unknown(),
  createdAt: z.string().datetime()
});

export type PersistedRunEventDto = z.infer<typeof PersistedRunEventDtoSchema>;

export const StreamModeDtoSchema = z.enum(["values", "updates", "debug", "messages"]);
export type StreamModeDto = z.infer<typeof StreamModeDtoSchema>;

const StateValueStreamEventSchema = z.object({
  type: z.literal("state_value"),
  state: z.object({
    runId: z.string().min(1),
    graphId: z.string().min(1),
    currentNodeId: z.string().min(1),
    status: z.enum(["idle", "running", "suspended", "completed", "failed", "cancelled"]),
    channels: z.record(z.string(), z.unknown()),
    version: z.number().int().min(0),
    checkpointId: z.string().min(1).optional(),
    createdAt: z.string().datetime(),
    updatedAt: z.string().datetime()
  })
});

const StateUpdateStreamEventSchema = z.object({
  type: z.literal("state_update"),
  delta: z.record(z.string(), z.unknown()),
  nodeId: z.string().min(1)
});

const MessageDeltaStreamEventSchema = z.object({
  type: z.literal("message_delta"),
  delta: z.string(),
  nodeId: z.string().min(1),
  messageId: z.string().min(1)
});

const ToolCallStreamEventSchema = z.object({
  type: z.literal("tool_call"),
  toolId: z.string().min(1),
  input: z.unknown(),
  nodeId: z.string().min(1)
});

const DebugStreamEventSchema = z.object({
  type: z.literal("debug"),
  payload: z.unknown(),
  nodeId: z.string().min(1)
});

export const StreamEventDtoSchema = z.discriminatedUnion("type", [
  StateValueStreamEventSchema,
  StateUpdateStreamEventSchema,
  MessageDeltaStreamEventSchema,
  ToolCallStreamEventSchema,
  DebugStreamEventSchema
]);

export type StreamEventDto = z.infer<typeof StreamEventDtoSchema>;
