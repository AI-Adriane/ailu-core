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
  it.each([
    { mapped: false, nested: true },
    { mapped: true, nested: false },
    { mapped: true, nested: true }
  ])(
    "governs every descendant before publishing ($mapped map, $nested nested)",
    async ({ mapped, nested }) => {
      const middle = {
        ...parentWithGatedChild,
        id: "middle",
        name: "middle",
        // Catalog descendants share the parent's node-id namespace.
        nodes: parentWithGatedChild.nodes.map((node) => ({ ...node, id: "middle-sub" })),
        entryNodeId: "middle-sub"
      } as GraphDefinition;
      const parent = {
        ...parentWithGatedChild,
        channels: {
          items: { type: "json", reducer: "replace" },
          results: { type: "json", reducer: "replace" }
        },
        nodes: [
          {
            id: "sub",
            type: "subgraph",
            label: "sub",
            subgraphId: nested ? "middle" : gatedChild.id,
            ...(mapped ? { mapSubgraph: { overChannel: "items", joinAt: "results" } } : {})
          }
        ]
      } as unknown as GraphDefinition;
      let published = 0;
      const engine = new InMemoryApprovalEngine();
      const runId = `descendant-${mapped}-${nested}`;
      const options = {
        subgraphs: nested ? [middle, gatedChild] : [gatedChild],
        nodes: [
          {
            id: "c_publish",
            execute: () => {
              published++;
              return {};
            }
          }
        ],
        approvalEngine: engine
      };
      const outcome = await runCatalogGraph(parent, {
        ...options,
        runId: runId as never,
        initialData: { items: [{}, {}] }
      });
      const pending = await engine.getPending();
      expect(pending).toHaveLength(mapped ? 2 : 1);
      for (const [index, request] of pending.entries()) {
        const childRun = `${runId}:sub${mapped ? `:${index}` : ""}${nested ? ":middle-sub" : ""}`;
        expect(request.runId).toBe(childRun);
        expect(request.nodeId).toBe(`${childRun}:c_gate`);
        expect(request.subject).toMatchObject({ description: `gate:${childRun}:c_gate` });
      }
      expect(outcome.state.channels.__approvalIds).toHaveLength(pending.length);
      await expect(resumeCatalogGraph(parent, outcome.state, options)).rejects.toMatchObject({
        code: "AILU_APPROVAL_NOT_GRANTED"
      });
      expect(published).toBe(0);
      await engine.approve(pending[0]!.id, "alice");
      if (mapped) {
        await expect(resumeCatalogGraph(parent, outcome.state, options)).rejects.toMatchObject({
          code: "AILU_APPROVAL_NOT_GRANTED"
        });
        expect(published).toBe(0);
        await engine.approve(pending[1]!.id, "bob");
      }
      const done = await resumeCatalogGraph(parent, outcome.state, options);
      expect(done.status).toBe("completed");
      expect(published).toBe(mapped ? 2 : 1);

      // Old checkpoints with no filed ids now fail closed through the public SDK too.
      const unfiled = await runCatalogGraph(parent, {
        ...options,
        approvalEngine: undefined,
        runId: `${runId}-unfiled` as never,
        initialData: { items: [{}, {}] }
      });
      await expect(resumeCatalogGraph(parent, unfiled.state, options)).rejects.toMatchObject({
        code: "AILU_APPROVAL_NOT_GRANTED"
      });
      expect(published).toBe(mapped ? 2 : 1);
    }
  );

  it("refuses replayed approval IDs from input, node output, and an unrelated saved record", async () => {
    const engine = new InMemoryApprovalEngine();
    const old = await engine.request({
      runId: "other-run" as never,
      nodeId: "gate" as never,
      requestedBy: "gate",
      subject: { description: "gate:gate" }
    });
    await engine.approve(old.id, "alice");
    let published = 0;
    const nodes = [
      { id: "draft", execute: async () => ({ __approvalIds: [old.id] }) },
      {
        id: "publish",
        execute: async () => {
          published++;
          return {};
        }
      }
    ];
    const outcome = await runCatalogGraph(gatedGraph, {
      runId: "current-run" as never,
      approvalEngine: engine,
      nodes,
      initialData: { __approvalIds: [old.id] }
    });
    const [pending] = await engine.getPending(outcome.state.runId);
    expect(pending).toBeDefined();
    expect(pending!.id).not.toBe(old.id);
    await expect(
      resumeCatalogGraph(gatedGraph, outcome.state, { approvalEngine: engine, nodes })
    ).rejects.toThrow(/still pending/);
    const replayed = {
      ...outcome.state,
      channels: { ...outcome.state.channels, __approvalIds: [old.id] }
    };
    await expect(
      resumeCatalogGraph(gatedGraph, replayed, { approvalEngine: engine, nodes })
    ).rejects.toThrow(/does not match/);
    expect(published).toBe(0);
    await engine.approve(pending!.id, "bob");
    const done = await resumeCatalogGraph(gatedGraph, outcome.state, {
      approvalEngine: engine,
      nodes
    });
    expect(done.status).toBe("completed");
    expect(published).toBe(1);
  });

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

    // A governed resume refuses to pass the gate while its request is pending, and files nothing.
    await expect(
      resumeCatalogGraph(gatedGraph, outcome.state, { approvalEngine: engine })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
    const pending = await engine.getPending(runId as never);
    expect(pending).toHaveLength(1);

    await engine.approve(pending[0]!.id, "alice");
    const resumed = await resumeCatalogGraph(gatedGraph, outcome.state, { approvalEngine: engine });
    expect(resumed.status).toBe("completed");
    expect(await engine.getPending(runId as never)).toHaveLength(0);
  });

  it("files a gate again when a loop comes back to it, and refuses to pass it on the old approval", async () => {
    // draft → gate → draft: every resume passes the gate, and the loop brings the run back to it.
    const loopGraph = {
      ...gatedGraph,
      id: "top-human-gate-loop",
      name: "top-human-gate-loop",
      edges: [
        { id: "e1", from: "draft", to: "gate", type: "default" },
        { id: "e2", from: "gate", to: "draft", type: "default" }
      ]
    } as unknown as GraphDefinition;
    const engine = new InMemoryApprovalEngine();
    const runId = "run_top_gate_loop";
    const first = await runCatalogGraph(loopGraph, {
      runId: runId as never,
      approvalEngine: engine
    });
    const [firstVisit] = await engine.getPending(runId as never);
    await engine.approve(firstVisit!.id, "alice");

    const second = await resumeCatalogGraph(loopGraph, first.state, { approvalEngine: engine });
    expect(second.status).toBe("suspended");
    expect(second.state.currentNodeId).toBe("gate");
    // The second visit is a new request for a person to decide; the first one's approval is spent.
    const pending = await engine.getPending(runId as never);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.subject).toMatchObject({ description: `${GATE_SUBJECT_PREFIX}gate` });
    expect(pending[0]?.id).not.toBe(firstVisit!.id);
    expect((second.state.channels as Record<string, unknown>).__approvalIds).toEqual([
      String(pending[0]!.id)
    ]);
    await expect(
      resumeCatalogGraph(loopGraph, second.state, { approvalEngine: engine })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
  });

  it("refuses a governed resume past a rejected gate", async () => {
    const engine = new InMemoryApprovalEngine();
    const outcome = await runCatalogGraph(gatedGraph, {
      runId: "run_top_gate_rejected" as never,
      approvalEngine: engine
    });
    const [request] = await engine.getPending(outcome.state.runId);
    await engine.reject(request!.id, "alice", "not this week");
    await expect(
      resumeCatalogGraph(gatedGraph, outcome.state, { approvalEngine: engine })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
  });

  it("refuses a governed resume of a state whose gate was never recorded", async () => {
    const engine = new InMemoryApprovalEngine();
    const ungoverned = await runCatalogGraph(gatedGraph, {
      runId: "run_top_gate_unrecorded" as never
    });
    expect(ungoverned.status).toBe("suspended");
    await expect(
      resumeCatalogGraph(gatedGraph, ungoverned.state, { approvalEngine: engine })
    ).rejects.toMatchObject({ code: "AILU_APPROVAL_NOT_GRANTED" });
  });
});
