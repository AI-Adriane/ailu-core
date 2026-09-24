import type { NodeId, RunId } from "@ailu-ai/graph-core";
import type { ArtifactRef } from "@ailu-ai/artifact-store";

import type { ApprovalId, ApprovalRequest } from "./types.js";

export type RequestApprovalParams = {
  runId: RunId;
  nodeId: NodeId;
  requestedBy: string;
  subject: ArtifactRef | { description: string };
  /**
   * Tenant the approval belongs to (ADR 0036 #4). Optional + back-compat: the engine itself is
   * tenant-agnostic and omits it; the CONTROL PLANE supplies it so an approval can be tenant-scoped
   * at the persistence layer (and so off-run gates — e.g. A2A outbound delegation — carry a tenant
   * even without a real run row). When set, an impl SHOULD persist + filter on it.
   */
  tenantId?: string;
};

export interface ApprovalEngine {
  request(params: RequestApprovalParams): Promise<ApprovalRequest>;
  approve(id: ApprovalId, resolvedBy: string): Promise<ApprovalRequest>;
  reject(id: ApprovalId, resolvedBy: string, reason: string): Promise<ApprovalRequest>;
  getPending(runId?: RunId): Promise<ApprovalRequest[]>;
  getById(id: ApprovalId): Promise<ApprovalRequest | undefined>;
}
