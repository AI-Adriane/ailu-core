import { z } from "zod";

export const NodeTypeSchema = z.enum(["action", "agent", "tool", "human-gate", "subgraph"]);
export const EdgeTypeSchema = z.enum(["default", "conditional"]);

const RetryPolicySchema = z.object({
  maxAttempts: z.number().int().min(1),
  backoffMs: z.number().int().min(0)
});

const NodeDefinitionDtoSchema = z.object({
  id: z.string().min(1),
  type: NodeTypeSchema,
  label: z.string().min(1),
  subgraphId: z.string().min(1).optional(),
  inputMapping: z.record(z.string(), z.string()).optional(),
  outputMapping: z.record(z.string(), z.string()).optional(),
  fanOut: z
    .object({
      parallelTo: z.array(z.string().min(1)).min(1),
      joinAt: z.string().min(1)
    })
    .optional(),
  retryPolicy: RetryPolicySchema.optional(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

const EdgeDefinitionDtoSchema = z.object({
  id: z.string().min(1),
  from: z.string().min(1),
  to: z.string().min(1),
  type: EdgeTypeSchema,
  condition: z.string().min(1).optional()
});

const ChannelReducerDtoSchema = z.enum(["replace", "append", "merge"]);

const ChannelDefinitionDtoSchema = z.object({
  type: z.string().min(1),
  reducer: ChannelReducerDtoSchema,
  default: z.unknown().optional(),
  /** ADR 0032: never emit this channel's value in run events/logs (masked; still checkpointed). */
  noLog: z.boolean().optional()
});

const ChannelsDtoSchema = z.record(z.string(), ChannelDefinitionDtoSchema);

export const CreateGraphDtoSchema = z.object({
  version: z.string().min(1),
  name: z.string().min(1),
  recursionLimit: z.number().int().min(1).optional(),
  channels: ChannelsDtoSchema,
  nodes: z.array(NodeDefinitionDtoSchema),
  edges: z.array(EdgeDefinitionDtoSchema),
  entryNodeId: z.string().min(1),
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const GraphDtoSchema = z.object({
  id: z.string().min(1),
  version: z.string().min(1),
  name: z.string().min(1),
  recursionLimit: z.number().int().min(1).optional(),
  channels: ChannelsDtoSchema,
  nodes: z.array(NodeDefinitionDtoSchema),
  edges: z.array(EdgeDefinitionDtoSchema),
  entryNodeId: z.string().min(1),
  metadata: z.record(z.string(), z.unknown()).optional(),
  /** Owning tenant (tenancy). Optional for pre-tenancy rows backfilled to `default`. */
  tenantId: z.string().min(1).optional(),
  /** Id of the principal who created the graph. */
  createdBy: z.string().min(1).optional()
});

export type NodeTypeDto = z.infer<typeof NodeTypeSchema>;
export type EdgeTypeDto = z.infer<typeof EdgeTypeSchema>;
export type NodeDefinitionDto = z.infer<typeof NodeDefinitionDtoSchema>;
export type EdgeDefinitionDto = z.infer<typeof EdgeDefinitionDtoSchema>;
export type ChannelDefinitionDto = z.infer<typeof ChannelDefinitionDtoSchema>;
export type ChannelsDto = z.infer<typeof ChannelsDtoSchema>;
export type CreateGraphDto = z.infer<typeof CreateGraphDtoSchema>;
export type GraphDto = z.infer<typeof GraphDtoSchema>;

/**
 * The serializable graph shape an example graph carries. Wire-compatible with the
 * engine `GraphDefinition` (version / name / channels / nodes / edges / entryNodeId),
 * carrying everything the Studio's reactflow preview and create-graph flow need —
 * without the Studio ever importing the engine or the SDK. The `id` is optional
 * because example definitions are authored in code, not yet persisted.
 */
export const ExampleGraphDefinitionDtoSchema = z.object({
  id: z.string().min(1).optional(),
  version: z.string().min(1),
  name: z.string().min(1),
  recursionLimit: z.number().int().min(1).optional(),
  channels: ChannelsDtoSchema,
  nodes: z.array(NodeDefinitionDtoSchema),
  edges: z.array(EdgeDefinitionDtoSchema),
  entryNodeId: z.string().min(1),
  metadata: z.record(z.string(), z.unknown()).optional()
});

/**
 * One example graph authored with `@ailu-ai/graph-sdk` and served by the control
 * plane so the (commercial) Studio can render it — a name/description, plus the
 * plain `definition` for the reactflow preview and the import-into-control-plane flow.
 */
export const ExampleGraphDtoSchema = z.object({
  slug: z.string().min(1),
  name: z.string().min(1),
  description: z.string().min(1),
  definition: ExampleGraphDefinitionDtoSchema
});

export type ExampleGraphDefinitionDto = z.infer<typeof ExampleGraphDefinitionDtoSchema>;
export type ExampleGraphDto = z.infer<typeof ExampleGraphDtoSchema>;
