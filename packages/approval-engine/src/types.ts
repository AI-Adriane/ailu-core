import type { NodeId, RunId } from "@ailu/graph-core";
import type { ArtifactRef } from "@ailu/artifact-store";

export type ApprovalId = string & { readonly __brand: "ApprovalId" };

export const APPROVAL_STATUSES = ["pending", "approved", "rejected"] as const;
export type ApprovalStatus = (typeof APPROVAL_STATUSES)[number];

export type ApprovalRequest = {
  id: ApprovalId;
  runId: RunId;
  nodeId: NodeId;
  requestedBy: string;
  subject: ArtifactRef | { description: string };
  status: ApprovalStatus;
  resolvedBy?: string;
  resolvedAt?: Date;
  rejectionReason?: string;
  createdAt: Date;
  /** Tenant scope (ADR 0036 #4) — set by the control plane; the engine is tenant-agnostic. */
  tenantId?: string;
};

export type ApprovalDecision = { approved: true } | { approved: false; reason: string };
