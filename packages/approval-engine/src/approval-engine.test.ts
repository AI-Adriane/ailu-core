import { describe, expect, it } from "vitest";
import type { NodeId, RunId } from "@ailu/graph-core";
import type { ArtifactRef } from "@ailu/artifact-store";

import {
  ApprovalAlreadyResolvedError,
  ApprovalSelfApprovalError
} from "./errors.js";
import { InMemoryApprovalEngine } from "./in-memory-approval-engine.js";

const runId = "run-1" as RunId;
const otherRunId = "run-2" as RunId;
const nodeId = "node-1" as NodeId;
const subject = { description: "Risky publish action" } satisfies { description: string };
const artifactSubject = { id: "artifact-1", version: 1 } as ArtifactRef;

describe("InMemoryApprovalEngine", () => {
  it("request creates a pending approval", async () => {
    const engine = new InMemoryApprovalEngine();

    const approval = await engine.request({
      runId,
      nodeId,
      requestedBy: "node:agent-review",
      subject
    });

    expect(approval.status).toBe("pending");
    expect(approval.requestedBy).toBe("node:agent-review");
  });

  it("approve resolves approval", async () => {
    const engine = new InMemoryApprovalEngine();
    const approval = await engine.request({
      runId,
      nodeId,
      requestedBy: "node:agent-review",
      subject: artifactSubject
    });

    const resolved = await engine.approve(approval.id, "user:alice");

    expect(resolved.status).toBe("approved");
    expect(resolved.resolvedBy).toBe("user:alice");
    expect(resolved.resolvedAt).toBeInstanceOf(Date);
  });

  it("reject resolves approval with reason", async () => {
    const engine = new InMemoryApprovalEngine();
    const approval = await engine.request({
      runId,
      nodeId,
      requestedBy: "node:agent-review",
      subject
    });

    const resolved = await engine.reject(approval.id, "user:bob", "Missing business context");

    expect(resolved.status).toBe("rejected");
    expect(resolved.rejectionReason).toBe("Missing business context");
  });

  it("self-approval throws ApprovalSelfApprovalError", async () => {
    const engine = new InMemoryApprovalEngine();
    const approval = await engine.request({
      runId,
      nodeId,
      requestedBy: "user:alice",
      subject
    });

    await expect(engine.approve(approval.id, "user:alice")).rejects.toBeInstanceOf(
      ApprovalSelfApprovalError
    );
  });

  it("double resolution throws ApprovalAlreadyResolvedError", async () => {
    const engine = new InMemoryApprovalEngine();
    const approval = await engine.request({
      runId,
      nodeId,
      requestedBy: "node:agent-review",
      subject
    });

    await engine.approve(approval.id, "user:alice");

    await expect(engine.reject(approval.id, "user:bob", "Too late")).rejects.toBeInstanceOf(
      ApprovalAlreadyResolvedError
    );
  });

  it("getPending filters by runId", async () => {
    const engine = new InMemoryApprovalEngine();
    await engine.request({
      runId,
      nodeId,
      requestedBy: "node:a",
      subject
    });
    const approvalToResolve = await engine.request({
      runId,
      nodeId,
      requestedBy: "node:b",
      subject
    });
    await engine.request({
      runId: otherRunId,
      nodeId,
      requestedBy: "node:c",
      subject
    });

    await engine.approve(approvalToResolve.id, "user:alice");

    const pendingAll = await engine.getPending();
    const pendingRun1 = await engine.getPending(runId);
    const pendingRun2 = await engine.getPending(otherRunId);

    expect(pendingAll).toHaveLength(2);
    expect(pendingRun1).toHaveLength(1);
    expect(pendingRun2).toHaveLength(1);
  });
});
