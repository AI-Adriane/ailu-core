import { z } from "zod/v3";

import { EDGE_TYPES, GRAPH_STATUSES, NODE_TYPES } from "./types";

const SemverPattern =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

const NodeIdSchema = z.string().min(1).brand<"NodeId">();
const EdgeIdSchema = z.string().min(1).brand<"EdgeId">();
const GraphIdSchema = z.string().min(1).brand<"GraphId">();
const RunIdSchema = z.string().min(1).brand<"RunId">();

const RetryPolicySchema = z.object({
  maxAttempts: z.number().int().min(1),
  backoffMs: z.number().int().min(0)
});

const NodeTypeSchema = z.enum(NODE_TYPES);
const EdgeTypeSchema = z.enum(EDGE_TYPES);
const GraphStatusSchema = z.enum(GRAPH_STATUSES);

export const NodeDefinitionSchema = z.object({
  id: NodeIdSchema,
  type: NodeTypeSchema,
  label: z.string().min(1),
  subgraphId: GraphIdSchema.optional(),
  inputMapping: z.record(z.string(), z.string()).optional(),
  outputMapping: z.record(z.string(), z.string()).optional(),
  fanOut: z
    .object({
      parallelTo: z.array(NodeIdSchema).min(1),
      joinAt: NodeIdSchema
    })
    .optional(),
  // ADR 0042 D2/D3 (product ADR 0068 — child workflows): dynamic N-child subgraph fan-out.
  mapSubgraph: z
    .object({
      overChannel: z.string().min(1),
      joinAt: z.string().min(1)
    })
    .optional(),
  retryPolicy: RetryPolicySchema.optional(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const EdgeDefinitionSchema = z
  .object({
    id: EdgeIdSchema,
    from: NodeIdSchema,
    to: NodeIdSchema,
    type: EdgeTypeSchema,
    condition: z.string().optional()
  })
  .superRefine((edge, ctx) => {
    const isConditional = edge.type === "conditional";
    const condition = edge.condition;
    const hasCondition = typeof condition === "string";

    if (isConditional && !hasCondition) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: "Conditional edges require a named condition string.",
        path: ["condition"]
      });
    }

    if (hasCondition && condition.trim().length === 0) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: "Condition must be a non-empty named string.",
        path: ["condition"]
      });
    }
  });

export const GraphStateSchema = z.object({
  runId: RunIdSchema,
  graphId: GraphIdSchema,
  currentNodeId: NodeIdSchema,
  status: GraphStatusSchema,
  channels: z.record(z.string(), z.unknown()),
  version: z.number().int().min(0),
  checkpointId: z.string().min(1).optional(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime()
});

export const GraphDefinitionSchema = z.object({
  id: GraphIdSchema,
  version: z.string().regex(SemverPattern, "Version must be a valid semver string."),
  name: z.string().min(1),
  recursionLimit: z.number().int().min(1).optional(),
  channels: z.record(
    z.string(),
    z.object({
      type: z.string().min(1),
      reducer: z.enum(["replace", "append", "merge"]),
      default: z.unknown().optional()
    })
  ),
  nodes: z.array(NodeDefinitionSchema),
  edges: z.array(EdgeDefinitionSchema),
  entryNodeId: NodeIdSchema,
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const ToolCallSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  input: z.unknown()
});

export const BaseMessageSchema = z.object({
  id: z.string().min(1),
  createdAt: z.date(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

export const HumanMessageSchema = BaseMessageSchema.extend({
  role: z.literal("human"),
  content: z.string()
});

export const AIMessageSchema = BaseMessageSchema.extend({
  role: z.literal("ai"),
  content: z.string(),
  toolCalls: z.array(ToolCallSchema).optional()
});

export const ToolMessageSchema = BaseMessageSchema.extend({
  role: z.literal("tool"),
  toolCallId: z.string().min(1),
  content: z.string()
});

export const SystemMessageSchema = BaseMessageSchema.extend({
  role: z.literal("system"),
  content: z.string()
});

export const MessageSchema = z.discriminatedUnion("role", [
  HumanMessageSchema,
  AIMessageSchema,
  ToolMessageSchema,
  SystemMessageSchema
]);
