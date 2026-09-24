import type { NodeId, RunId } from "@ailu-ai/graph-core";
import { describe, expect, it } from "vitest";

import {
  Ed25519Attestor,
  canonicalJson,
  verifyAttestation,
  verifyChain,
  type AttestationRecord
} from "./attestation.js";
import type { ApprovalId, ApprovalRequest } from "./types.js";

const resolved = (over?: Partial<ApprovalRequest>): ApprovalRequest => ({
  id: "approval-1" as ApprovalId,
  runId: "run-1" as RunId,
  nodeId: "assistant" as NodeId,
  requestedBy: "assistant",
  subject: { description: "tool:refund" },
  status: "approved",
  resolvedBy: "alice",
  resolvedAt: new Date("2026-06-10T12:00:00.000Z"),
  createdAt: new Date("2026-06-10T11:59:00.000Z"),
  ...over
});

describe("approval attestation", () => {
  it("canonicalJson is stable regardless of key order", () => {
    expect(canonicalJson({ b: 1, a: 2 })).toBe(canonicalJson({ a: 2, b: 1 }));
    expect(canonicalJson({ a: [3, { y: 1, x: 2 }] })).toBe('{"a":[3,{"x":2,"y":1}]}');
  });

  it("signs a decision and verifies it", () => {
    const attestor = new Ed25519Attestor();
    const record = attestor.attest(resolved());

    expect(record.algorithm).toBe("ed25519");
    expect(record.status).toBe("approved");
    expect(record.subject).toBe("tool:refund");
    expect(verifyAttestation(record)).toBe(true);
  });

  it("fails verification if any attested field is altered", () => {
    const attestor = new Ed25519Attestor();
    const record = attestor.attest(resolved());

    expect(verifyAttestation({ ...record, resolvedBy: "mallory" })).toBe(false);
    expect(verifyAttestation({ ...record, status: "rejected" })).toBe(false);
    expect(verifyAttestation({ ...record, payloadHash: "deadbeef" })).toBe(false);
  });

  it("fails verification if the signature is tampered", () => {
    const attestor = new Ed25519Attestor();
    const record = attestor.attest(resolved());
    const flipped = Buffer.from(record.signature, "base64");
    flipped[0] = (flipped[0] ?? 0) ^ 0xff;
    expect(verifyAttestation({ ...record, signature: flipped.toString("base64") })).toBe(false);
  });

  it("chains records and verifies linkage", () => {
    const attestor = new Ed25519Attestor();
    const first = attestor.attest(resolved({ id: "approval-1" as ApprovalId }));
    const second = attestor.attest(
      resolved({ id: "approval-2" as ApprovalId, subject: { description: "tool:wire_transfer" } }),
      first.payloadHash
    );

    expect(second.prevHash).toBe(first.payloadHash);
    expect(verifyChain([first, second])).toBe(true);
  });

  it("rejects a reordered or broken chain", () => {
    const attestor = new Ed25519Attestor();
    const first = attestor.attest(resolved({ id: "approval-1" as ApprovalId }));
    const second = attestor.attest(resolved({ id: "approval-2" as ApprovalId }), first.payloadHash);

    // Swapped order breaks the prevHash linkage.
    expect(verifyChain([second, first])).toBe(false);
    // A record spliced out breaks the chain too.
    const orphan: AttestationRecord = { ...second, prevHash: "deadbeef" };
    expect(verifyChain([first, orphan])).toBe(false);
  });

  it("refuses to attest a pending approval", () => {
    const attestor = new Ed25519Attestor();
    expect(() => attestor.attest(resolved({ status: "pending", resolvedBy: undefined }))).toThrow();
  });
});
