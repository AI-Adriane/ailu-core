import { describe, expect, it } from "vitest";

import { ListRunsQueryDtoSchema, PatchRunStateDtoSchema, RESERVED_PATCH_CHANNELS, RunDtoSchema } from "./runs.js";

/**
 * The patch-run-state contract gates the `PATCH /runs/:id/state` boundary. A client
 * may write ordinary channels, but never the reserved governance channels
 * (`__approvedTools` / `__approvalIds`) — writing those directly would be a
 * self-approval back door (forge an unlock, or erase the pending ids the resume gate
 * reads). The schema rejects such a patch so it surfaces as a 400/422, not a silent
 * state mutation.
 */
describe("@ailu-ai/contracts — PatchRunStateDtoSchema reserved-channel gate", () => {
  it("accepts a patch of ordinary channels", () => {
    const result = PatchRunStateDtoSchema.safeParse({
      patch: { draft: "hello", count: 3 },
      resumeFrom: "node-a"
    });
    expect(result.success).toBe(true);
  });

  it("rejects a patch that writes __approvedTools", () => {
    const result = PatchRunStateDtoSchema.safeParse({
      patch: { __approvedTools: ["refund"] }
    });
    expect(result.success).toBe(false);
    if (!result.success) {
      expect(result.error.issues[0]?.message).toContain("__approvedTools");
    }
  });

  it("rejects a patch that writes __approvalIds", () => {
    const result = PatchRunStateDtoSchema.safeParse({
      patch: { __approvalIds: [] }
    });
    expect(result.success).toBe(false);
    if (!result.success) {
      expect(result.error.issues[0]?.message).toContain("__approvalIds");
    }
  });

  it("rejects a patch that mixes a reserved channel with ordinary ones", () => {
    const result = PatchRunStateDtoSchema.safeParse({
      patch: { draft: "ok", __approvedTools: ["wire"] }
    });
    expect(result.success).toBe(false);
  });

  it("names both reserved channels", () => {
    expect([...RESERVED_PATCH_CHANNELS]).toEqual(["__approvedTools", "__approvalIds"]);
  });
});

/**
 * ADR 0044 — `cancelled` is a first-class terminal run status on the wire. A control plane that
 * stops a run must be able to say so WITHOUT lying: not "failed" (nothing malfunctioned) and not
 * "rejected" (no gate was refused). These tests pin that the published contract accepts it
 * everywhere a status appears, since the whole point is that the distinction survives the API
 * boundary.
 */
describe("@ailu-ai/contracts — cancelled run status", () => {
  const baseRun = {
    id: "graph-support:run:1",
    graphId: "graph-support",
    channels: {},
    version: 3,
    createdAt: "2026-09-16T10:15:00.000Z",
    updatedAt: "2026-09-16T10:15:42.000Z"
  };

  it("accepts a cancelled run on RunDtoSchema", () => {
    const result = RunDtoSchema.safeParse({ ...baseRun, status: "cancelled" });
    expect(result.success).toBe(true);
  });

  it("keeps cancelled distinct from failed and rejected", () => {
    for (const status of ["cancelled", "failed", "rejected"]) {
      expect(RunDtoSchema.safeParse({ ...baseRun, status }).success).toBe(true);
    }
    // A status outside the vocabulary is still refused — the enum did not become a free string.
    expect(RunDtoSchema.safeParse({ ...baseRun, status: "aborted" }).success).toBe(false);
  });

  it("lets the run list be filtered by status=cancelled", () => {
    const result = ListRunsQueryDtoSchema.safeParse({ status: "cancelled" });
    expect(result.success).toBe(true);
  });
});
