import { describe, expect, it } from "vitest";

import { attestedDecisions } from "./index.js";

/** A link of the chain, reduced to what attestedDecisions reads. */
const link = (approvalId: string, subject: string, callKey?: string, runId = "run-1") => ({
  runId,
  approvalId,
  subject,
  ...(callKey !== undefined ? { callKey } : {})
});

describe("attestedDecisions", () => {
  it("keeps one decision per request, in chain order, with the call it signed", () => {
    expect(
      attestedDecisions([link("a-1", "tool:refund", "refund#abc"), link("a-2", "gate:approve")])
    ).toEqual([
      { status: "", subject: "tool:refund", callKey: "refund#abc" },
      { status: "", subject: "gate:approve" }
    ]);
  });

  it("reads a quorum's signatures as one decision: its resolution, the last link", () => {
    expect(
      attestedDecisions([
        link("q-1", "gate:approve"),
        link("q-1", "gate:approve"),
        link("q-1", "gate:approve")
      ])
    ).toEqual([{ status: "", subject: "gate:approve" }]);
  });

  it("places a request at its resolution, after a request resolved in between", () => {
    expect(
      attestedDecisions([
        link("q-1", "gate:legal"),
        link("a-2", "tool:refund"),
        link("q-1", "gate:legal")
      ])
    ).toEqual([
      { status: "", subject: "tool:refund" },
      { status: "", subject: "gate:legal" }
    ]);
  });

  it("never hides a signed tool decision behind a gate of the same id (P1)", () => {
    expect(
      attestedDecisions([
        link("x", "tool:wire_transfer", "wire_transfer#beef"),
        link("x", "gate:approve")
      ])
    ).toEqual([
      { status: "", subject: "tool:wire_transfer", callKey: "wire_transfer#beef" },
      { status: "", subject: "gate:approve" }
    ]);
  });

  it("never merges links of the same id that name different gates (P2)", () => {
    expect(attestedDecisions([link("x", "gate:legal"), link("x", "gate:approve")])).toEqual([
      { status: "", subject: "gate:legal" },
      { status: "", subject: "gate:approve" }
    ]);
  });

  it("keeps links of another call or another run apart", () => {
    expect(
      attestedDecisions([
        link("x", "tool:refund", "refund#1"),
        link("x", "tool:refund", "refund#2"),
        link("y", "gate:approve", undefined, "run-1"),
        link("y", "gate:approve", undefined, "run-2")
      ])
    ).toHaveLength(4);
  });

  it("attests nothing for an empty chain", () => {
    expect(attestedDecisions([])).toEqual([]);
  });
});
