import { z } from "zod";

import { ModelTierDtoSchema } from "./catalog.js";

/**
 * The SHARED CARRIER for catalog-backed graph nodes.
 *
 * A graph node that is a catalog COMPONENT carries `node.metadata.component =
 * { kind, params }`; a catalog AGENT node carries `node.metadata.agent =
 * { provider?, model?, tier?, system?, toolNames?, maxIterations?,
 *   suspendForApproval?, approvalToolNames?, outputChannel?, outputStyle?,
 *   contextBudget?, todosChannel?, enableFs?, resolvedMiddleware? }`.
 *
 * The graph editor EMITS these into `node.metadata`; the API run path READS
 * `node.metadata` to assemble `EngineSpec.componentNodes` (kind + params) and
 * `EngineSpec.agents` (the agent spec). Typing the carrier here keeps it
 * end-to-end typed across the API ↔ Studio boundary even though the underlying
 * `NodeDefinitionDto.metadata` stays an open `Record<string, unknown>`.
 */

/** A catalog COMPONENT carrier: a `kind` plus its parameter bag. */
export const ComponentNodeMetadataSchema = z.object({
  /** One of `ComponentRegistry::kinds()` (e.g. "promptBuilder") or an integration name. */
  kind: z.string().min(1),
  /** The component's parameter bag; validated by the native factory at assemble time. */
  params: z.record(z.string(), z.unknown())
});

/** A catalog AGENT carrier: mirrors the engine `AgentSpec` (all fields optional in the editor). */
export const AgentNodeMetadataSchema = z.object({
  provider: z.string().min(1).optional(),
  model: z.string().min(1).optional(),
  tier: ModelTierDtoSchema.optional(),
  system: z.string().optional(),
  toolNames: z.array(z.string().min(1)).optional(),
  maxIterations: z.number().int().min(1).optional(),
  suspendForApproval: z.boolean().optional(),
  approvalToolNames: z.array(z.string().min(1)).optional(),
  outputChannel: z.string().min(1).optional(),
  /** ADR 0014 — terse output directive on the system prompt. */
  outputStyle: z.literal("terse").optional(),
  /** ADR 0014 — cap (chars) on the agent's seed message (the injected `Input`/`State` dump). */
  contextBudget: z.number().int().min(1).optional(),
  /** ADR 0022/0023 — durable channel the agent's `writeTodos` list is persisted into. */
  todosChannel: z.string().min(1).optional(),
  /** ADR 0030 phase 9e — channel carrying the run's multimodal input blocks. */
  inputBlocksChannel: z.string().min(1).optional(),
  /** ADR 0026 phase 11 — governed long-term memory overlay (namespace tenant-scoped). */
  memory: z
    .object({
      namespace: z.string().min(1),
      topK: z.number().int().min(1).optional(),
      recall: z.enum(["vector", "graph", "both"]).optional()
    })
    .optional(),
  /** ADR 0035 phase 12 — governed skills overlay (progressive disclosure; namespace tenant-scoped). */
  skills: z
    .object({
      namespace: z.string().min(1),
      /** Explicit `name@version` pins — the must-apply playbooks. */
      required: z.array(z.string().min(1)).optional(),
      /** Cap on advisory (vector-selected) skills. */
      advisoryK: z.number().int().min(0).optional()
    })
    .optional(),
  /** ADR 0024 phase 2c/2d — opt this agent into the governed virtual filesystem tools. */
  enableFs: z.boolean().optional(),
  /**
   * ADR 0075 (issue #566 G3) — attach an external MCP server's tools to this agent, mid-run. The
   * value references a tenant-owned `connector_connections` row (`provider: "mcp"`), resolved and
   * connected at run start; every discovered tool is unconditionally `requiresApproval` (an MCP
   * server is code Ailu does not control and cannot statically vet). Distinct from `toolNames`/
   * `approvalToolNames` above — those are re-derived at run start to include the tools this
   * connection discovers, since the tool set isn't known until the server is actually queried.
   */
  mcpConnectionId: z.string().min(1).optional(),
  /**
   * Issue #566 G19 — attach one or more governed action-tool connectors to this agent, mid-run.
   * Keyed by provider id (e.g. `"slack"`), value is a tenant-owned `connector_connections` row id.
   * Same resolution shape as `mcpConnectionId` (control plane resolves the connection, discovers
   * that provider's action tools, merges them into `toolNames`/`approvalToolNames` before the run
   * reaches this carrier — the Rust engine never resolves a connection itself), generalized to a
   * map so adding the NEXT provider (Linear, Salesforce, ...) never needs another engine field/
   * release — only a new product-side adapter keyed into the same map.
   */
  actionConnections: z.record(z.string().min(1), z.string().min(1)).optional(),
  /**
   * ADR 0025 phase 3d — the resolved EFFICIENCY middleware list. A discriminated union of
   * efficiency-only kinds (compress / terse / contextBudget): a GOVERNANCE kind (redact /
   * approvalGate / fsPolicy) fails this schema BY CONSTRUCTION, so any consumer that runs it
   * (e.g. `readAgentMetadata`) drops the malformed agent. This is a **type-level + validation
   * guarantee** for callers that opt to validate; it is NOT auto-applied on the persisted
   * catalog run path today (that path reads an unvalidated carrier — the RUNTIME enforcer
   * there is the Rust bridge, whose match only honours efficiency kinds and ignores governance
   * kinds). The SDK's `toRustAgentConfig` throw-gate covers the in-process builder path.
   */
  resolvedMiddleware: z
    .array(
      z.discriminatedUnion("kind", [
        z.object({ kind: z.literal("compress") }),
        z.object({ kind: z.literal("terse") }),
        z.object({
          kind: z.literal("contextBudget"),
          params: z.object({ chars: z.number().int().min(1) })
        }),
        z.object({
          kind: z.literal("reflection"),
          params: z.object({ threshold: z.number().min(0).max(1).optional() }).optional()
        }),
        // ADR 0029 phase 8 — structured output (efficiency kind; the schema rides in params).
        z.object({
          kind: z.literal("structuredOutput"),
          params: z.object({
            schema: z.record(z.unknown()),
            name: z.string().optional(),
            strict: z.boolean().optional(),
            mode: z.enum(["required", "lenient"]).optional(),
            retryCap: z.number().int().min(0).optional()
          })
        })
      ])
    )
    .optional()
});

/**
 * A catalog MAP-AGENTS carrier (ADR 0027 phase 4b — dynamic fan-out): run `subAgent` once per item
 * in the `overChannel` array and collect the per-item results (in input order) into `joinAt`. The
 * sub-agent is itself an agent carrier (same shape as a plain agent node → deepagents parity: skills,
 * memory, fs, planning all apply to each spawn). Mirrors the engine `MapAgentSpec`.
 */
export const MapAgentNodeMetadataSchema = z.object({
  /** Channel holding the array of items to fan the sub-agent out over. */
  overChannel: z.string().min(1),
  /** Channel the array of per-item results lands in (one entry per item, in input order). */
  joinAt: z.string().min(1),
  /** The sub-agent run per item — a full agent carrier. */
  subAgent: AgentNodeMetadataSchema,
  /** When true, a spawn that needs approval suspends the whole map (default false). */
  suspendForApproval: z.boolean().optional()
});

/**
 * The slice of `node.metadata` the catalog carrier owns. All keys are optional;
 * a node carries at most one (a component OR an agent OR a mapAgents fan-out).
 * Other metadata keys are untouched.
 */
export const CatalogNodeMetadataSchema = z
  .object({
    component: ComponentNodeMetadataSchema.optional(),
    agent: AgentNodeMetadataSchema.optional(),
    mapAgents: MapAgentNodeMetadataSchema.optional()
  })
  .passthrough();

export type ComponentNodeMetadata = z.infer<typeof ComponentNodeMetadataSchema>;
export type AgentNodeMetadata = z.infer<typeof AgentNodeMetadataSchema>;
export type MapAgentNodeMetadata = z.infer<typeof MapAgentNodeMetadataSchema>;
export type CatalogNodeMetadata = z.infer<typeof CatalogNodeMetadataSchema>;

/** Narrow an open metadata bag to its component carrier, if present and valid. */
export const readComponentMetadata = (
  metadata: Record<string, unknown> | undefined
): ComponentNodeMetadata | undefined => {
  if (metadata === undefined) {
    return undefined;
  }
  const parsed = ComponentNodeMetadataSchema.safeParse(metadata.component);
  return parsed.success ? parsed.data : undefined;
};

/** Narrow an open metadata bag to its agent carrier, if present and valid. */
export const readAgentMetadata = (
  metadata: Record<string, unknown> | undefined
): AgentNodeMetadata | undefined => {
  if (metadata === undefined) {
    return undefined;
  }
  const parsed = AgentNodeMetadataSchema.safeParse(metadata.agent);
  return parsed.success ? parsed.data : undefined;
};

/** Narrow an open metadata bag to its mapAgents (dynamic fan-out) carrier, if present and valid. */
export const readMapAgentMetadata = (
  metadata: Record<string, unknown> | undefined
): MapAgentNodeMetadata | undefined => {
  if (metadata === undefined) {
    return undefined;
  }
  const parsed = MapAgentNodeMetadataSchema.safeParse(metadata.mapAgents);
  return parsed.success ? parsed.data : undefined;
};
