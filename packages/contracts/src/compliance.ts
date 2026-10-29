import { z } from "zod/v3";

/** EU AI Act risk tiers (simplified): minimal | limited | high. */
export const ComplianceRiskSchema = z.enum(["minimal", "limited", "high"]);

/** One graph in the tenant cartography, with its AI-system inventory + risk classification. */
export const ComplianceGraphEntrySchema = z.object({
  id: z.string().min(1),
  name: z.string(),
  nodeCount: z.number().int().min(0),
  agentNodes: z.array(
    z.object({
      nodeId: z.string(),
      provider: z.string().optional(),
      model: z.string().optional()
    })
  ),
  hasHumanGate: z.boolean(),
  risk: ComplianceRiskSchema
});

/** A knowledge base's data-quality summary: volume + freshness window. */
export const ComplianceKbEntrySchema = z.object({
  namespace: z.string(),
  documents: z.number().int().min(0),
  oldest: z.string().optional(),
  newest: z.string().optional()
});

/** Tenant-level AI Act cartography + aggregate human-oversight stats. */
export const ComplianceTenantReportDtoSchema = z.object({
  tenantId: z.string(),
  graphs: z.array(ComplianceGraphEntrySchema),
  providers: z.array(z.string()),
  knowledgeBases: z.array(ComplianceKbEntrySchema),
  supervision: z.object({
    totalApprovals: z.number().int().min(0),
    resolved: z.number().int().min(0),
    selfApprovalViolations: z.number().int().min(0)
  }),
  generatedAt: z.string().datetime()
});

/** One lifecycle event in a run's decision trail. */
export const ComplianceEventSchema = z.object({
  seq: z.number().int(),
  type: z.string(),
  nodeId: z.string().nullable(),
  at: z.string()
});

/** One human-approval record proving oversight (distinctResolver = no self-approval). */
export const ComplianceApprovalSchema = z.object({
  id: z.string(),
  nodeId: z.string(),
  subject: z.string(),
  requestedBy: z.string(),
  resolvedBy: z.string().nullable(),
  distinctResolver: z.boolean(),
  status: z.string(),
  resolvedAt: z.string().nullable()
});

/** A data source the run relied on, with provenance (the KB doc + its origin/freshness). */
export const ComplianceSourceSchema = z.object({
  id: z.string(),
  namespace: z.string().optional(),
  resource: z.string().optional(),
  timestamp: z.string().optional()
});

/** Per-run AI Act report: decision traceability + human supervision + data provenance. */
/** PII-handling evidence for the run (ADR 0008): detections, redactions, blocks. */
export const CompliancePiiEventSchema = z.object({
  type: z.string(),
  count: z.number().int().optional(),
  timestamp: z.string()
});

export const ComplianceRunReportDtoSchema = z.object({
  runId: z.string(),
  graphId: z.string(),
  status: z.string(),
  risk: ComplianceRiskSchema,
  traceability: z.array(ComplianceEventSchema),
  humanSupervision: z.array(ComplianceApprovalSchema),
  dataSources: z.array(ComplianceSourceSchema),
  piiEvents: z.array(CompliancePiiEventSchema),
  generatedAt: z.string().datetime()
});

export type ComplianceRisk = z.infer<typeof ComplianceRiskSchema>;
export type ComplianceGraphEntry = z.infer<typeof ComplianceGraphEntrySchema>;
export type ComplianceKbEntry = z.infer<typeof ComplianceKbEntrySchema>;
export type ComplianceTenantReportDto = z.infer<typeof ComplianceTenantReportDtoSchema>;
export type ComplianceEvent = z.infer<typeof ComplianceEventSchema>;
export type ComplianceApproval = z.infer<typeof ComplianceApprovalSchema>;
export type ComplianceSource = z.infer<typeof ComplianceSourceSchema>;
export type CompliancePiiEvent = z.infer<typeof CompliancePiiEventSchema>;
export type ComplianceRunReportDto = z.infer<typeof ComplianceRunReportDtoSchema>;
