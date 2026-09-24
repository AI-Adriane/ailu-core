import { describe, expect, it } from "vitest";
// Import the in-memory engine directly (not the package index) so the test never
// pulls the Pg engine and its `db`/`pg` dependency chain — same discipline as
// governance-enforcement.test.ts.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

import { resumeCatalogGraph, runCatalogGraph, rustEngineAvailable, type GraphDefinition } from "./index.js";

/**
 * ADR 0042 D2/D3 (product ADR 0068 D5.4, ailu-engine#177) — the falsifiable
 * approval-non-inheritance test.
 *
 * History: this test originally proved a GAP, not the intended property —
 * `fileApprovalRequests` walked only the top-level `definition.nodes`, so a child's
 * gated tool call filed ZERO `ApprovalEngine` requests at all (tracked as #177). Fixed
 * by having `fileApprovalRequests` also walk a DIRECT (non-fan-out) child's own nodes,
 * reading its `approvalRequests` from the nested `__subgraphStates[childRunId].channels`
 * snapshot `execute_subgraph` already writes on suspend. This test now proves that
 * property directly — `engine.request()` is called for the child's own tool call.
 *
 * A first pass of this fix (still #177) kept `nodeId`/`requestedBy` qualified with the
 * child's run id but left `runId` itself pointed at the PARENT — a review of the
 * product ADR 0068 Revision 10 design (which assumed a genuinely child-scoped
 * `ApprovalEngine.getPending(childRunId)`/attestation chain) caught that the request was
 * therefore filed, signed, and persisted under the PARENT's `runId`, making
 * `loadAttestationChain(childRunId)` return nothing — not attributable to the child at
 * all. Fixed here too: `runId` passed to `engine.request()` is now the child's own
 * deterministic run id, exactly like `nodeId`/`requestedBy` already were.
 *
 * Deliberately unchanged: a nested subgraph inside a subgraph, and `mapSubgraph`'s
 * dynamic N-child fan-out, are NOT walked — same scope D5.3's own run-gate injection
 * chose (no precomputable single id for either case). The second test below is a
 * non-regression marker for that boundary, not a claim those cases are safe.
 */
const gatedChild: GraphDefinition = {
  id: "sub-gated-tool",
  version: "1",
  name: "sub-gated-tool",
  channels: {
    agentResult: { type: "agentResult", reducer: "replace" },
    __approvedTools: { type: "string[]", reducer: "replace", default: [] },
    __approvalIds: { type: "string[]", reducer: "replace", default: [] }
  },
  nodes: [
    {
      id: "c_assistant",
      type: "agent",
      label: "c_assistant",
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
  entryNodeId: "c_assistant"
} as unknown as GraphDefinition;

const parentWithGatedChild: GraphDefinition = {
  id: "parent-with-gated-child",
  version: "1",
  name: "parent-with-gated-child",
  channels: {},
  nodes: [
    {
      id: "sub",
      type: "subgraph",
      label: "sub",
      subgraphId: "sub-gated-tool",
      inputMapping: {},
      outputMapping: {}
    }
  ],
  edges: [],
  entryNodeId: "sub"
} as unknown as GraphDefinition;

/** A parent whose subgraph reference fans out (`mapSubgraph`) — out of this fix's scope. */
const parentWithGatedMapSubgraph: GraphDefinition = {
  id: "parent-with-gated-map-subgraph",
  version: "1",
  name: "parent-with-gated-map-subgraph",
  channels: {
    items: { type: "array", reducer: "replace", default: [] },
    results: { type: "array", reducer: "replace", default: [] }
  },
  nodes: [
    {
      id: "sub",
      type: "subgraph",
      label: "sub",
      subgraphId: "sub-gated-tool",
      inputMapping: {},
      outputMapping: {},
      mapSubgraph: { overChannel: "items", joinAt: "results" }
    }
  ],
  edges: [],
  entryNodeId: "sub"
} as unknown as GraphDefinition;

const rustOnly = rustEngineAvailable() ? describe : describe.skip;

rustOnly(
  "@ailu/graph-sdk — child-workflow approval filing (ADR 0042 D2/D3, product ADR 0068 D5.4)",
  () => {
    it(
      "files an ApprovalEngine request for a DIRECT child's gated tool call, under a " +
        "child-qualified grant key distinct from the parent's own node id",
      async () => {
        const engine = new InMemoryApprovalEngine();
        const runId = "run_child_approval_fixed";
        const outcome = await runCatalogGraph(parentWithGatedChild, {
          runId: runId as never,
          subgraphs: [gatedChild],
          approvalEngine: engine
        });

        expect(outcome.status).toBe("suspended");

        // The deterministic child run id is `<parentRunId>:<nodeId>` (subgraph_run_id,
        // runtime.rs) — the request is filed under THAT id (not the parent's), so its
        // ApprovalEngine attestation is genuinely attributable to the child, not merely
        // labelled as one. A lookup under the PARENT's own runId finds nothing.
        const childRunId = `${runId}:sub`;
        expect(await engine.getPending(runId as never)).toHaveLength(0);
        const pending = await engine.getPending(childRunId as never);
        expect(pending).toHaveLength(1);
        expect(pending[0]?.requestedBy).toBe(`${childRunId}:c_assistant`);
        expect(pending[0]?.subject).toMatchObject({ description: "tool:refund" });

        const ids = (outcome.state.channels as Record<string, unknown>).__approvalIds;
        expect(Array.isArray(ids)).toBe(true);
        expect(ids as string[]).toHaveLength(1);
        expect((ids as string[])[0]).toBe(String(pending[0]!.id));
      }
    );

    it(
      "does not double-file when an already-governed suspended state is driven through " +
        "the seam again (e.g. a resume with nothing approved yet, re-suspending at the SAME gate)",
      async () => {
        const engine = new InMemoryApprovalEngine();
        const runId = "run_child_approval_idempotent";
        const outcome = await runCatalogGraph(parentWithGatedChild, {
          runId: runId as never,
          subgraphs: [gatedChild],
          approvalEngine: engine
        });
        expect(outcome.status).toBe("suspended");
        const childRunId = `${runId}:sub`;
        expect(await engine.getPending(childRunId as never)).toHaveLength(1);

        // Resuming with nothing approved re-suspends at the exact same child gate — the
        // returned state already carries the stashed __approvalIds, so the guard must
        // skip filing again rather than creating a second request for the same decision.
        const resumed = await resumeCatalogGraph(parentWithGatedChild, outcome.state, {
          subgraphs: [gatedChild],
          approvalEngine: engine
        });
        expect(resumed.status).toBe("suspended");
        expect(await engine.getPending(childRunId as never)).toHaveLength(1);
      }
    );

    it(
      "NON-REGRESSION: a mapSubgraph (fan-out) child's gate is still NOT filed — out of " +
        "this fix's scope, same as D5.3's own run-gate injection boundary",
      async () => {
        const engine = new InMemoryApprovalEngine();
        const runId = "run_child_approval_mapsubgraph_unscoped";
        const outcome = await runCatalogGraph(parentWithGatedMapSubgraph, {
          runId: runId as never,
          initialData: { items: [{}] },
          subgraphs: [gatedChild],
          approvalEngine: engine
        });
        expect(outcome.status).toBe("suspended");
        expect(await engine.getPending(runId as never)).toHaveLength(0);
      }
    );
  }
);
