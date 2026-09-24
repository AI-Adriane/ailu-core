import type { RunId } from "@ailu-ai/graph-core";

import type { ApprovalEngine, RequestApprovalParams } from "./interfaces.js";
import type { ApprovalId, ApprovalRequest } from "./types.js";
import {
  ApprovalAlreadyResolvedError,
  ApprovalNotFoundError,
  ApprovalSelfApprovalError
} from "./errors.js";

const createApprovalId = (): ApprovalId =>
  `approval-${Date.now()}-${Math.random().toString(36).slice(2, 8)}` as ApprovalId;

export class InMemoryApprovalEngine implements ApprovalEngine {
  private readonly approvals = new Map<ApprovalId, ApprovalRequest>();

  public async request(params: RequestApprovalParams): Promise<ApprovalRequest> {
    const approval: ApprovalRequest = {
      id: createApprovalId(),
      runId: params.runId,
      nodeId: params.nodeId,
      requestedBy: params.requestedBy,
      subject: params.subject,
      status: "pending",
      createdAt: new Date(),
      tenantId: params.tenantId
    };

    this.approvals.set(approval.id, approval);
    return approval;
  }

  public async approve(id: ApprovalId, resolvedBy: string): Promise<ApprovalRequest> {
    const approval = this.getOrThrow(id);
    this.ensureCanResolve(approval, resolvedBy);

    const resolved: ApprovalRequest = {
      ...approval,
      status: "approved",
      resolvedBy,
      resolvedAt: new Date(),
      rejectionReason: undefined
    };

    this.approvals.set(id, resolved);
    return resolved;
  }

  public async reject(id: ApprovalId, resolvedBy: string, reason: string): Promise<ApprovalRequest> {
    const approval = this.getOrThrow(id);
    this.ensureCanResolve(approval, resolvedBy);

    const resolved: ApprovalRequest = {
      ...approval,
      status: "rejected",
      resolvedBy,
      resolvedAt: new Date(),
      rejectionReason: reason
    };

    this.approvals.set(id, resolved);
    return resolved;
  }

  public async getPending(runId?: RunId): Promise<ApprovalRequest[]> {
    const pending = [...this.approvals.values()].filter((approval) => approval.status === "pending");
    if (runId === undefined) {
      return pending;
    }

    return pending.filter((approval) => approval.runId === runId);
  }

  public async getById(id: ApprovalId): Promise<ApprovalRequest | undefined> {
    return this.approvals.get(id);
  }

  private getOrThrow(id: ApprovalId): ApprovalRequest {
    const approval = this.approvals.get(id);
    if (approval === undefined) {
      throw new ApprovalNotFoundError(id);
    }

    return approval;
  }

  private ensureCanResolve(approval: ApprovalRequest, resolvedBy: string): void {
    if (approval.status !== "pending") {
      throw new ApprovalAlreadyResolvedError(approval.id);
    }

    if (approval.requestedBy === resolvedBy) {
      throw new ApprovalSelfApprovalError(approval.id);
    }
  }
}
