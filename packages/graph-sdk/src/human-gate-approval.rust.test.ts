import { describe, expect, it } from "vitest";
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

import {
  GATE_SUBJECT_PREFIX,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition
} from "./index.js";

/**
 * ADR 0068 issue #496 — a `human-gate` node suspends the run (`execute_node` returns
 * `self.suspend(...)` unconditionally) but, unlike an agent's `suspendForApproval`, it
 * carries no `approvalRequests` payload of its own — before this fix, `resume()`'s ONLY
 * authorization check (`ensureNoPendingApprovals`, reading `ApprovalEngine.getPending`)
 * saw nothing pending for a `human-gate` suspension and would let a resume through with
 * NO decision ever recorded. The gate delayed; it did not authorize. This applies to
 * D5.3's own injected `__run_gate` node too, not just an authored `human-gate`.
 *
 * Fixed by having `fileApprovalRequests` file one request for the suspended node itself
 * when its type is `human-gate` (top-level or a direct child's), under a `"gate:"`-
 * prefixed subject so the control plane can tell a rejected GATE apart from a rejected
 * TOOL when deciding whether a run becomes `"rejected"` (a tool rejection just leaves a
 * tool unlocked; a gate rejection must block resume outright — that product-side check
 * is issue #496's own follow-up, not this engine-side filing fix).
 *
 * A review of the product ADR 0068 Revision 10 design (which assumed a genuinely
 * child-scoped attestation chain, `loadAttestationChain(childRunId)`) caught that a
 * child's gate request was filed under the PARENT's own `runId` (only `nodeId`/
 * `requestedBy` were child-qualified) — not attributable to the child at all. Fixed
 * alongside the same bug in `fileForGraphNodes` (child tool-call gates): `runId` passed
 * to `engine.request()` for a child's own gate is now the child's deterministic run id.
 */
const gatedGraph: GraphDefinition = {
  id: "top-human-gate",
  version: "1",
  name: "top-human-gate",
  channels: {},
  nodes: [
    { id: "draft", type: "action", label: "draft" },
    { id: "gate", type: "human-gate", label: "gate" },
    { id: "publish", type: "action", label: "publish" }
  ],
  edges: [
    { id: "e1", from: "draft", to: "gate", type: "default" },
    { id: "e2", from: "gate", to: "publish", type: "default" }
  ],
  entryNodeId: "draft"
} as unknown as GraphDefinition;

const gatedChild: GraphDefinition = {
  id: "sub-human-gate",
  version: "1",
  name: "sub-human-gate",
  channels: {},
  nodes: [
    { id: "c_draft", type: "action", label: "c_draft" },
    { id: "c_gate", type: "human-gate", label: "c_gate" },
    { id: "c_publish", type: "action", label: "c_publish" }
  ],
  edges: [
    { id: "e1", from: "c_draft", to: "c_gate", type: "default" },
    { id: "e2", from: "c_gate", to: "c_publish", type: "default" }
  ],
  entryNodeId: "c_draft"
} as unknown as GraphDefinition;

const parentWithGatedChild: GraphDefinition = {
  id: "parent-with-human-gate-child",
  version: "1",
  name: "parent-with-human-gate-child",
  channels: {},
  nodes: [
    {
      id: "sub",
      type: "subgraph",
      label: "sub",
      subgraphId: "sub-human-gate",
      inputMapping: {},
      outputMapping: {}
    }
  ],
  edges: [],
  entryNodeId: "sub"
} as unknown as GraphDefinition;

const rustOnly = rustEngineAvailable() ? describe : describe.skip;

rustOnly("@ailu-ai/graph-sdk — human-gate approval filing (product ADR 0068, issue #496)", () => {
  it("files an ApprovalEngine request for a TOP-LEVEL human-gate suspension", async () => {
    const engine = new InMemoryApprovalEngine();
    const runId = "run_top_gate";
    const outcome = await runCatalogGraph(gatedGraph, {
      runId: runId as never,
      approvalEngine: engine
    });

    expect(outcome.status).toBe("suspended");
    const pending = await engine.getPending(runId as never);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.requestedBy).toBe("gate");
    expect(pending[0]?.subject).toMatchObject({ description: `${GATE_SUBJECT_PREFIX}gate` });

    const ids = (outcome.state.channels as Record<string, unknown>).__approvalIds;
    expect((ids as string[])[0]).toBe(String(pending[0]!.id));
  });

  it("files an ApprovalEngine request for a DIRECT child's own human-gate suspension", async () => {
    const engine = new InMemoryApprovalEngine();
    const runId = "run_child_gate";
    const outcome = await runCatalogGraph(parentWithGatedChild, {
      runId: runId as never,
      subgraphs: [gatedChild],
      approvalEngine: engine
    });

    expect(outcome.status).toBe("suspended");
    // Filed under the CHILD's own run id — a lookup under the parent's finds nothing.
    const childRunId = `${runId}:sub`;
    expect(await engine.getPending(runId as never)).toHaveLength(0);
    const pending = await engine.getPending(childRunId as never);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.requestedBy).toBe(`${childRunId}:c_gate`);
    expect(pending[0]?.subject).toMatchObject({
      description: `${GATE_SUBJECT_PREFIX}${childRunId}:c_gate`
    });
  });

  it("does not double-file when an already-governed suspended gate state is re-driven", async () => {
    const engine = new InMemoryApprovalEngine();
    const runId = "run_top_gate_idempotent";
    const outcome = await runCatalogGraph(gatedGraph, {
      runId: runId as never,
      approvalEngine: engine
    });
    expect(outcome.status).toBe("suspended");
    expect(await engine.getPending(runId as never)).toHaveLength(1);

    // The engine's OWN resume() unconditionally advances past a human-gate — this proves
    // only that the FILING guard doesn't double-file on a re-drive, not that advancing
    // here is a safe thing for a real caller to do (the control plane must never call
    // resume() before the filed gate request is resolved — issue #496's own follow-up).
    const resumed = await resumeCatalogGraph(gatedGraph, outcome.state, { approvalEngine: engine });
    expect(resumed.status).toBe("completed");
    expect(await engine.getPending(runId as never)).toHaveLength(1);
  });
});
