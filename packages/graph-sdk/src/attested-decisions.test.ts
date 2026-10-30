import { describe, expect, it } from "vitest";

import { attestedDecisions } from "./index.js";

/** A link of the chain, reduced to what attestedDecisions reads. */
const link = (approvalId: string, subject: string, callKey?: string, runId = "run-1") => ({
  runId,
  approvalId,
  subject,
  ...(callKey !== undefined ? { callKey } : {})
});

/** A signed link of a gate, with its signer and status, and optionally its grant. */
const signed = (
  resolvedBy: string,
  extra: {
    status?: "approved" | "rejected";
    grant?: string;
    subject?: string;
    callKey?: string;
  } = {}
) => ({
  runId: "run-1",
  approvalId: "gate-1",
  subject: extra.subject ?? "gate:level4",
  callKey: extra.callKey ?? `release#${"a".repeat(64)}`,
  resolvedBy,
  status: extra.status ?? "approved",
  ...(extra.grant !== undefined ? { grant: extra.grant } : {})
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

  describe("a gate signed by several members (a quorum)", () => {
    const callKey = `release#${"a".repeat(64)}`;
    const one = [{ status: "", subject: "gate:level4", callKey }];

    it("reads two approvals by distinct signers of one request as one decision", () => {
      expect(attestedDecisions([signed("first"), signed("second")])).toEqual(one);
    });

    it("reads them as one decision when only the resolution names the grant", () => {
      expect(attestedDecisions([signed("first"), signed("second", { grant: callKey })])).toEqual(
        one
      );
    });

    it("keeps a gate signed by one member as one decision", () => {
      expect(attestedDecisions([signed("only")])).toEqual(one);
    });

    it("does not hide a signer who signed twice behind the last link", () => {
      expect(attestedDecisions([signed("first"), signed("first")])).toHaveLength(2);
    });

    it("does not hide a link that signs another status", () => {
      expect(
        attestedDecisions([signed("first", { status: "rejected" }), signed("second")])
      ).toHaveLength(2);
    });

    it("does not hide a link of another call or another subject under the same id", () => {
      expect(
        attestedDecisions([
          signed("first", { callKey: `release#${"b".repeat(64)}` }),
          signed("second")
        ])
      ).toHaveLength(2);
      expect(
        attestedDecisions([signed("first", { subject: "gate:legal" }), signed("second")])
      ).toHaveLength(2);
    });

    it("does not hide a link that names another grant", () => {
      expect(
        attestedDecisions([
          signed("first", { grant: "release" }),
          signed("second", { grant: callKey })
        ])
      ).toHaveLength(2);
    });
  });
});
