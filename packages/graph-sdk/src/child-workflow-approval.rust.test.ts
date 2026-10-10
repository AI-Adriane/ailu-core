import { describe, expect, it } from "vitest";
// Import the in-memory engine directly (not the package index) so the test never
// pulls the Pg engine and its `db`/`pg` dependency chain — same discipline as
// governance-enforcement.test.ts.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

import {
  ApprovalRefusedError,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition
} from "./index.js";

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
 * ADR 0045 rev. 1 R6 (2.7): a grant cannot reach a child run — the bridge writes
 * `__approvedTools` into the top-level state only, and the child resumes from its own snapshot —
 * so a filed, approved child request looped the run (the agent asked again on every resume). Such
 * a wait is now refused: nothing is filed, and `ApprovalRefusedError` tells the host to fail the
 * run with its reason. A short follow-up revision routes the grant into the child.
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
  "@ailu-ai/graph-sdk — child-workflow approval filing (ADR 0042 D2/D3, product ADR 0068 D5.4)",
  () => {
    it(
      "refuses a DIRECT child's gated tool call instead of filing it — no grant can reach a " +
        "child run yet, so approving it would loop the run (ADR 0045 rev. 1 R6)",
      async () => {
        const engine = new InMemoryApprovalEngine();
        const runId = "run_child_approval_refused";
        const refused = await runCatalogGraph(parentWithGatedChild, {
          runId: runId as never,
          subgraphs: [gatedChild],
          approvalEngine: engine
        }).catch((error: unknown) => error);

        expect(refused).toBeInstanceOf(ApprovalRefusedError);
        const error = refused as ApprovalRefusedError;
        expect(error.code).toBe("AILU_APPROVAL_REFUSED");
        expect(error.reason).toBe(
          "a tool approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6)"
        );
        // The run stopped where it waits; nothing was filed for a person to sign in vain.
        expect(error.state.status).toBe("suspended");
        // The run's outcome rides along, so the host keeps what the run did (its journal).
        expect(error.outcome?.status).toBe("suspended");
        expect(error.outcome?.state).toBe(error.state);
        expect(error.outcome?.usedRustEngine).toBe(true);
        // The run's channels are never serialized with the error (PII): only code and reason.
        expect(Object.keys(error)).not.toContain("state");
        expect(Object.keys(error)).not.toContain("outcome");
        const serialized = JSON.stringify(error);
        expect(serialized).toContain("AILU_APPROVAL_REFUSED");
        expect(serialized).not.toContain("__subgraphStates");
        expect(await engine.getPending(runId as never)).toHaveLength(0);
        expect(await engine.getPending(`${runId}:sub` as never)).toHaveLength(0);
      }
    );

    it(
      "refuses to resume such a wait, before reading the approval engine — a state kept " +
        "before 2.7 included",
      async () => {
        // Ungoverned, the run just suspends at the child's call (nothing changes without an
        // approval engine).
        const outcome = await runCatalogGraph(parentWithGatedChild, {
          runId: "run_child_approval_kept" as never,
          subgraphs: [gatedChild]
        });
        expect(outcome.status).toBe("suspended");

        const engine = new InMemoryApprovalEngine();
        await expect(
          resumeCatalogGraph(parentWithGatedChild, outcome.state, {
            subgraphs: [gatedChild],
            approvalEngine: engine
          })
        ).rejects.toMatchObject({
          code: "AILU_APPROVAL_REFUSED",
          reason: "a tool approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6)",
          // Refused before anything ran: no outcome to keep.
          outcome: undefined
        });
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
