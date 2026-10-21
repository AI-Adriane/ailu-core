import { afterEach, describe, expect, it } from "vitest";

import {
  InMemoryApprovalEngine,
  callKeyOf,
  exampleGraphs,
  replayCatalogGraph,
  replayedDecisions,
  runCatalogGraph,
  rustEngineAvailable,
  verifyReplayDecisions,
  type GraphDefinition,
  type GraphState,
  type RunId
} from "./index.js";

/**
 * Replay-as-evidence (ADR 0038): the decisions a replay re-derives are the requests a run files
 * where the replay stopped (`filing_plan`, ADR 0045 D3.1) — the human gate it stops at included,
 * which a replay's `pendingApprovals` never name. The cases first mirror the control plane's own
 * rule (ailu #1586), then the real engine: run → sign → replay → the same decisions.
 */

const native = rustEngineAvailable() ? it : it.skip;

// Skipped without the native engine — explicitly, and counted — unless CI requires it.
it.runIf(process.env.AILU_REQUIRE_NATIVE === "1")(
  "runs on the native engine (AILU_REQUIRE_NATIVE=1)",
  () => {
    expect(rustEngineAvailable()).toBe(true);
  }
);

/** An agent that may ask for gated tools, a human gate, then an action. */
const graph = {
  id: "replayed-decisions",
  version: "1",
  name: "replayed-decisions",
  channels: {},
  nodes: [
    {
      id: "draft",
      type: "agent",
      label: "draft",
      metadata: {
        agent: { provider: "anthropic", toolNames: ["refund", "payout"], suspendForApproval: true }
      }
    },
    { id: "approve", type: "human-gate", label: "approve" },
    { id: "send", type: "action", label: "send" }
  ],
  edges: [],
  entryNodeId: "draft"
} as unknown as GraphDefinition;

/** A replay's outcome: stopped at `currentNodeId`, the agent's output listing `requests`. */
const stop = (
  status: string,
  currentNodeId: string | undefined,
  requests: unknown[] = [],
  approvalIds: string[] = []
): { status: string; state: GraphState } => ({
  status,
  state: {
    runId: "run-1",
    graphId: "replayed-decisions",
    ...(currentNodeId !== undefined ? { currentNodeId } : {}),
    status,
    channels: {
      agentResult: { approvalRequests: requests },
      ...(approvalIds.length > 0 ? { __approvalIds: approvalIds } : {})
    },
    version: 3,
    checkpointId: "run-1:3",
    createdAt: "0",
    updatedAt: "0"
  } as unknown as GraphState
});

describe("replayedDecisions", () => {
  native("keeps what an agent requested, in order, each with the call it asks for", () => {
    const refund = { amount: 600, order: "ORD-8830" };
    expect(
      replayedDecisions(
        stop("suspended", "draft", [
          { subject: "tool:refund", input: refund, reason: "needs approval" },
          { subject: "tool:payout", reason: "needs approval" }
        ]),
        graph
      )
    ).toEqual([
      { status: "", subject: "tool:refund", callKey: callKeyOf("refund", JSON.stringify(refund)) },
      { status: "", subject: "tool:payout" }
    ]);
  });

  native("names the human gate a replay stops at — the request a run files there", () => {
    expect(replayedDecisions(stop("suspended", "approve"), graph)).toEqual([
      { status: "", subject: "gate:approve" }
    ]);
  });

  native("lists an agent's tools, then the gate — the order a run files them", () => {
    expect(
      replayedDecisions(
        stop("suspended", "approve", [{ subject: "tool:refund", reason: "needs approval" }]),
        graph
      )
    ).toEqual([
      { status: "", subject: "tool:refund" },
      { status: "", subject: "gate:approve" }
    ]);
  });

  native("names the gate once, whatever request ids the state carried from its recording", () => {
    expect(replayedDecisions(stop("suspended", "approve", [], ["filed-0"]), graph)).toEqual([
      { status: "", subject: "gate:approve" }
    ]);
  });

  native("adds nothing for a stop that is not a human gate", () => {
    expect(replayedDecisions(stop("suspended", "send"), graph)).toEqual([]);
    expect(replayedDecisions(stop("suspended", undefined), graph)).toEqual([]);
  });

  it("re-derives nothing for a replay that did not suspend", () => {
    expect(replayedDecisions(stop("completed", "approve"), graph)).toEqual([]);
    expect(
      replayedDecisions(
        stop("failed", "draft", [{ subject: "tool:refund", reason: "needs approval" }]),
        graph
      )
    ).toEqual([]);
  });
});

/** A component assembles a draft, a human gate approves it (the control plane's content flow). */
const gated = {
  id: "gated-draft",
  version: "0.0.0",
  name: "gated-draft",
  channels: {
    docType: { type: "string", reducer: "replace", default: "" },
    body: { type: "string", reducer: "replace", default: "" }
  },
  nodes: [
    {
      id: "assemble",
      type: "action",
      label: "assemble",
      metadata: {
        component: { kind: "promptBuilder", params: { into: "body", template: "{{body}}" } }
      }
    },
    { id: "approve", type: "human-gate", label: "Approve the draft" }
  ],
  edges: [{ id: "e_assemble_approve", from: "assemble", to: "approve", type: "default" }],
  entryNodeId: "assemble"
} as unknown as GraphDefinition;

describe("replayedDecisions on the real engine", () => {
  afterEach(() => {
    delete process.env.AILU_LLM_RECORD;
  });

  for (const decision of ["approve", "reject"] as const) {
    native(`reproduces the human gate a member signed (${decision})`, async () => {
      process.env.AILU_LLM_RECORD = "1";
      const approvals = new InMemoryApprovalEngine();
      const runId = `replayed-gate-${decision}` as RunId;
      const started = await runCatalogGraph(gated, {
        runId,
        initialData: { body: "The draft.", docType: "memo" },
        approvalEngine: approvals
      });
      expect(started.status).toBe("suspended");
      const [filed] = await approvals.getPending(runId);
      if (filed === undefined) throw new Error("the gate filed no request");
      if (decision === "approve") await approvals.approve(filed.id, "member");
      else await approvals.reject(filed.id, "member", "not this one");
      if (!("description" in filed.subject))
        throw new Error("the gate's request has no description");
      const attested = [{ status: "", subject: filed.subject.description }];

      const replay = await replayCatalogGraph(
        gated,
        started.entryState ?? started.state,
        `${runId}:entry`,
        started.replayJournal ?? "{}"
      );
      // The engine reports no pending request at a human gate…
      expect(replay.pendingApprovals ?? []).toEqual([]);
      // …the replay's decisions name it, so the comparison holds.
      const replayed = replayedDecisions(replay, gated);
      expect(replayed).toEqual(attested);
      expect(verifyReplayDecisions(attested, replayed).ok).toBe(true);
      // A chain that signs a gate the run never reached is still not reproduced.
      expect(
        verifyReplayDecisions([{ status: "", subject: "gate:some-other-gate" }], replayed).ok
      ).toBe(false);
    });
  }

  native("re-derives an agent's gated tool call as the replay requests it", async () => {
    process.env.AILU_LLM_RECORD = "1";
    const definition = exampleGraphs().find((g) => g.slug === "approval-demo")?.definition;
    if (definition === undefined) throw new Error("approval-demo example graph not found");
    const started = await runCatalogGraph(definition, {
      initialData: { input: "Refund ORD-8830 (duplicate charge)." },
      approvalEngine: new InMemoryApprovalEngine()
    });
    const replay = await replayCatalogGraph(
      definition,
      started.entryState ?? started.state,
      "replayed-tool:entry",
      started.replayJournal ?? "{}"
    );
    const requested = (replay.pendingApprovals ?? []).map((pending) => ({
      status: "",
      subject: pending.subject,
      ...(pending.callKey !== undefined ? { callKey: pending.callKey } : {})
    }));
    expect(requested.length).toBeGreaterThan(0);
    expect(replayedDecisions(replay, definition)).toEqual(requested);
  });
});
