import { describe, expect, it } from "vitest";
// Import the in-memory engine directly (not the package index) so the test never
// pulls the Pg engine and its `db`/`pg` dependency chain.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

import {
  APPROVAL_IDS_CHANNEL,
  isCatalogGraph,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition
} from "./index.js";

// The SDK runs exclusively on the Rust engine (the TS fallback was removed). The
// governed-approval authority is the **catalog seam**: an agent node carried entirely in
// `node.metadata.agent` runs natively on Rust, and the seam files an ApprovalEngine
// request per gated tool (control-plane authority — never a TS handler). The
// `ApprovalEngine` itself enforces the no-self-approval invariant (tested in the
// `@ailu-ai/approval-engine` package); these tests assert the Rust seam wiring.

// ---------------------------------------------------------------------------
// Catalog (Rust) path: emission via the seam, non-duplication, determinism.
// ---------------------------------------------------------------------------

/** A catalog graph: a single gated agent node carried entirely in node.metadata.agent. */
const catalogGatedGraph = (): GraphDefinition =>
  ({
    id: "gov-catalog",
    version: "0.0.0",
    name: "gov-catalog",
    channels: {
      agentResult: { type: "agentResult", reducer: "replace" },
      __approvedTools: { type: "string[]", reducer: "replace", default: [] },
      __approvalIds: { type: "string[]", reducer: "replace", default: [] }
    },
    nodes: [
      {
        id: "assistant",
        type: "agent",
        label: "assistant",
        metadata: {
          agent: {
            provider: "anthropic",
            toolNames: ["refund"],
            suspendForApproval: true,
            approvalToolNames: ["refund"],
            outputChannel: "agentResult"
          }
        }
      }
    ],
    edges: [],
    entryNodeId: "assistant"
  }) as unknown as GraphDefinition;

const rustOnly = rustEngineAvailable() ? describe : describe.skip;

rustOnly("@ailu-ai/graph-sdk governance — catalog seam emission (Rust engine)", () => {
  it("recognizes the gated agent graph as a catalog graph", () => {
    expect(isCatalogGraph(catalogGatedGraph())).toBe(true);
  });

  it("files exactly one ApprovalEngine request per gated tool and stashes its id", async () => {
    const engine = new InMemoryApprovalEngine();
    const outcome = await runCatalogGraph(catalogGatedGraph(), {
      runId: "run_cat_1" as never,
      approvalEngine: engine
    });

    expect(outcome.status).toBe("suspended");

    // Emission happened in the SEAM (control-plane authority), not duplicated: exactly
    // one pending request, filed under the node id as requester.
    const pending = await engine.getPending("run_cat_1" as never);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.requestedBy).toBe("assistant");
    expect(pending[0]?.subject).toMatchObject({ description: "tool:refund" });

    // The engine id is stashed in the run's __approvalIds channel.
    const ids = (outcome.state.channels as Record<string, unknown>)[APPROVAL_IDS_CHANNEL];
    expect(Array.isArray(ids)).toBe(true);
    expect(ids as string[]).toHaveLength(1);
    expect((ids as string[])[0]).toBe(String(pending[0]!.id));
  });

  it("does not double-file: emission happens in the seam, never also via a TS handler", async () => {
    // The catalog path runs the agent NATIVELY on Rust — there is no TS
    // createAgentNodeHandler in play — so the only emission point is the seam. One
    // suspended run yields exactly one pending request (catalog XOR fallback emission).
    const engine = new InMemoryApprovalEngine();
    const outcome = await runCatalogGraph(catalogGatedGraph(), {
      runId: "run_cat_2" as never,
      approvalEngine: engine
    });
    expect(outcome.status).toBe("suspended");
    const pending = await engine.getPending("run_cat_2" as never);
    expect(pending).toHaveLength(1);
    const ids = (outcome.state.channels as Record<string, unknown>)[APPROVAL_IDS_CHANNEL];
    expect((ids as string[]).length).toBe(1); // one id stashed, matching the one request
  });

  it("an ungoverned catalog run (no engine) files nothing", async () => {
    const engine = new InMemoryApprovalEngine();
    const outcome = await runCatalogGraph(catalogGatedGraph(), {
      runId: "run_cat_3" as never
      // no approvalEngine — legacy channel-only behaviour
    });
    expect(outcome.status).toBe("suspended");
    expect((await engine.getPending("run_cat_3" as never)).length).toBe(0);
    const ids = (outcome.state.channels as Record<string, unknown>)[APPROVAL_IDS_CHANNEL];
    expect(ids === undefined || (ids as string[]).length === 0).toBe(true);
  });
});

rustOnly("@ailu-ai/graph-sdk governance — each new suspension is filed", () => {
  const twoGates = (): GraphDefinition =>
    ({
      id: "two-gates",
      version: "0.0.0",
      name: "two-gates",
      channels: { __approvalIds: { type: "string[]", reducer: "replace", default: [] } },
      nodes: [
        { id: "legal", type: "human-gate", label: "legal" },
        { id: "finance", type: "human-gate", label: "finance" }
      ],
      edges: [{ id: "e1", from: "legal", to: "finance", type: "default" }],
      entryNodeId: "legal"
    }) as unknown as GraphDefinition;

  it("files the second gate after the first one is approved and resumed", async () => {
    const engine = new InMemoryApprovalEngine();
    const definition = twoGates();

    const first = await runCatalogGraph(definition, { approvalEngine: engine });
    const [legal] = await engine.getPending(first.state.runId);
    expect(first.status).toBe("suspended");
    expect(legal).toBeDefined();
    await engine.approve(legal!.id, "alice");

    const second = await resumeCatalogGraph(definition, first.state, { approvalEngine: engine });
    expect(second.status).toBe("suspended");
    expect(String(second.state.currentNodeId)).toBe("finance");
    const pending = await engine.getPending(second.state.runId);
    expect(pending).toHaveLength(1);
    expect(pending[0]!.id).not.toBe(legal!.id);
    expect(second.state.channels[APPROVAL_IDS_CHANNEL]).toEqual([pending[0]!.id]);
  });
});

rustOnly("@ailu-ai/graph-sdk governance — a governed resume needs the engine's approval", () => {
  const suspendedForRefund = async (engine: InMemoryApprovalEngine, runId: string) => {
    const outcome = await runCatalogGraph(catalogGatedGraph(), {
      runId: runId as never,
      approvalEngine: engine
    });
    expect(outcome.status).toBe("suspended");
    const [request] = await engine.getPending(runId as never);
    return { state: outcome.state, request: request! };
  };
  const refundGrant = (resolvedBy: string) => [
    { name: "refund", requestedBy: "assistant", resolvedBy }
  ];

  it("refuses while the tool request is pending", async () => {
    const engine = new InMemoryApprovalEngine();
    const { state } = await suspendedForRefund(engine, "run_gov_pending");
    await expect(
      resumeCatalogGraph(catalogGatedGraph(), state, {
        approvalEngine: engine,
        approvedTools: refundGrant("alice")
      })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
  });

  it("refuses a grant the engine recorded as rejected", async () => {
    const engine = new InMemoryApprovalEngine();
    const { state, request } = await suspendedForRefund(engine, "run_gov_rejected");
    await engine.reject(request.id, "alice", "too large");
    await expect(
      resumeCatalogGraph(catalogGatedGraph(), state, {
        approvalEngine: engine,
        approvedTools: refundGrant("alice")
      })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
  });

  it("refuses a grant that names another approver than the engine recorded", async () => {
    const engine = new InMemoryApprovalEngine();
    const { state, request } = await suspendedForRefund(engine, "run_gov_other_approver");
    await engine.approve(request.id, "alice");
    const refused = await resumeCatalogGraph(catalogGatedGraph(), state, {
      approvalEngine: engine,
      approvedTools: refundGrant("mallory")
    }).catch((error: unknown) => error);
    expect(refused).toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
    expect((refused as { problems: string[] }).problems[0]).toContain("refund");
  });

  it("resumes once the engine approved the tool, by the approver the grant names", async () => {
    const engine = new InMemoryApprovalEngine();
    const { state, request } = await suspendedForRefund(engine, "run_gov_approved");
    await engine.approve(request.id, "alice");
    const done = await resumeCatalogGraph(catalogGatedGraph(), state, {
      approvalEngine: engine,
      approvedTools: refundGrant("alice")
    });
    expect(done.status).toBe("completed");
  });
});
