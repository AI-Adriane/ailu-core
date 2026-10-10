import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import type { ApprovalEngine, ApprovalId, ApprovalRequest } from "@ailu-ai/approval-engine";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApprovalNotGrantedError,
  ApprovalRefusedError,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition,
  type GraphState
} from "./index.js";

/**
 * ADR 0045 D3.1 — golden cases for the approvals of a catalog run. Two kinds:
 *
 * - `filing`: a run (or a resume from `previousState`) returned `state`; `expected` is what the
 *   TypeScript SDK filed in the approval engine, in order, and the `__approvalIds` it kept;
 * - `check`: a governed resume of `state` with `approvedTools`, the engine holding `approvals`;
 *   `expected` is the ids the SDK read back, in order, and the problems it refused the resume
 *   for (empty when it let it go on).
 *
 * Recorded from the TypeScript SDK (2.2.0) through its public API, with the engine's run entries
 * stubbed to return each case's state, before the engine took these decisions over. The file is
 * frozen: the SDK must keep deciding the same, now through the engine — and so must the engine's
 * functions, from TypeScript, from Rust (`crates/runtime-bridge` tests) and from Python. A
 * deliberate change edits the file by hand.
 */
const FIXTURE = fileURLToPath(
  new URL(
    "../../../crates/runtime-bridge/tests/fixtures/catalog_approvals_golden.json",
    import.meta.url
  )
);

type FilingCase = {
  name: string;
  kind: "filing";
  input: { graph: unknown; subgraphs?: unknown[]; state: unknown; previousState?: unknown };
  expected: { requests: unknown[]; approvalIds: unknown; refusal?: string };
};
type CheckCase = {
  name: string;
  kind: "check";
  input: {
    graph: unknown;
    subgraphs?: unknown[];
    state: unknown;
    approvedTools?: { name: string; requestedBy: string; resolvedBy: string }[];
    approvals: Record<string, unknown>;
  };
  expected: { reads: string[]; problems: string[] };
};
type GoldenCase = FilingCase | CheckCase;

const napi = (): {
  engineRun: (...args: unknown[]) => Promise<string>;
  engineResume: (...args: unknown[]) => Promise<string>;
  engineCatalogApprovalPlan: (inputJson: string) => string;
  engineCatalogApprovalsToCheck: (stateJson: string) => string;
  engineCatalogResumeProblems: (inputJson: string) => string;
} => createRequire(import.meta.url)("@ailu-ai/napi");

/** The engine's own decision for a case, in the golden shape (the recorder's ids are `filed-<n>`). */
const engineResult = (golden: GoldenCase): GoldenCase["expected"] => {
  if (golden.kind === "filing") {
    const plan = JSON.parse(napi().engineCatalogApprovalPlan(JSON.stringify(golden.input))) as {
      clearApprovalIds: boolean;
      requests: unknown[];
      refusal?: string;
    };
    const kept = plan.clearApprovalIds ? [] : ids(golden.input.state);
    return {
      requests: plan.requests,
      approvalIds: plan.requests.length > 0 ? plan.requests.map((_, n) => `filed-${n}`) : kept,
      // ADR 0045 rev. 1 R6: a refused plan says why; every other case keeps its shape.
      ...(plan.refusal === undefined ? {} : { refusal: plan.refusal })
    };
  }
  return {
    reads: JSON.parse(
      napi().engineCatalogApprovalsToCheck(JSON.stringify(golden.input.state))
    ) as string[],
    problems: JSON.parse(
      napi().engineCatalogResumeProblems(JSON.stringify(golden.input))
    ) as string[]
  };
};

const outcomeOf = (state: unknown): string =>
  JSON.stringify({ state, status: (state as { status: string }).status, pendingApprovals: [] });

/** An approval engine that records what is filed and read, answering reads from `records`. */
const recorder = (records: Record<string, unknown> = {}, approveUnknown = false) => {
  const filed: unknown[] = [];
  const reads: string[] = [];
  const engine: ApprovalEngine = {
    request: async (params) => {
      filed.push({
        runId: String(params.runId),
        nodeId: String(params.nodeId),
        requestedBy: params.requestedBy,
        subject: params.subject
      });
      return {
        ...params,
        id: `filed-${filed.length - 1}` as ApprovalId,
        status: "pending",
        createdAt: new Date(0)
      } as ApprovalRequest;
    },
    approve: async () => {
      throw new Error("not used");
    },
    reject: async () => {
      throw new Error("not used");
    },
    getPending: async () => [],
    getById: async (id) => {
      reads.push(String(id));
      if (String(id) in records) {
        return (records[String(id)] ?? undefined) as ApprovalRequest | undefined;
      }
      return approveUnknown
        ? ({
            id,
            status: "approved",
            subject: { description: "gate:any" },
            requestedBy: "a-node",
            resolvedBy: "alice"
          } as unknown as ApprovalRequest)
        : undefined;
    }
  };
  return { engine, filed, reads };
};

const ids = (state: unknown): unknown =>
  ((state as GraphState).channels as Record<string, unknown>).__approvalIds ?? null;

/** What the SDK files for a filing case. */
const sdkFiling = async (input: FilingCase["input"]): Promise<FilingCase["expected"]> => {
  const state = input.state as GraphState;
  const subgraphs = input.subgraphs as GraphDefinition[] | undefined;
  if (input.previousState === undefined) {
    const { engine, filed } = recorder();
    vi.spyOn(napi(), "engineRun").mockImplementation(async () => outcomeOf(state));
    try {
      const outcome = await runCatalogGraph(input.graph as GraphDefinition, {
        runId: state.runId,
        approvalEngine: engine,
        subgraphs
      });
      return { requests: filed, approvalIds: ids(outcome.state) };
    } catch (error) {
      // ADR 0045 rev. 1 R6: a wait no person can decide is refused, with its reason.
      if (error instanceof ApprovalRefusedError) {
        return { requests: filed, approvalIds: ids(error.state), refusal: error.reason };
      }
      throw error;
    }
  }
  // The resume's own check reads the previous ids back: the engine approved them all.
  const { engine, filed } = recorder({}, true);
  vi.spyOn(napi(), "engineResume").mockImplementation(async () => outcomeOf(state));
  const outcome = await resumeCatalogGraph(
    input.graph as GraphDefinition,
    input.previousState as GraphState,
    { approvalEngine: engine, subgraphs }
  );
  return { requests: filed, approvalIds: ids(outcome.state) };
};

/** What the SDK decides for a check case. */
const sdkCheck = async (input: CheckCase["input"]): Promise<CheckCase["expected"]> => {
  const { engine, reads } = recorder(input.approvals);
  const state = input.state as GraphState;
  vi.spyOn(napi(), "engineResume").mockImplementation(async () =>
    outcomeOf({ ...state, status: "completed" })
  );
  try {
    await resumeCatalogGraph(input.graph as GraphDefinition, state, {
      approvalEngine: engine,
      approvedTools: input.approvedTools ?? [],
      subgraphs: input.subgraphs as GraphDefinition[] | undefined
    });
    return { reads, problems: [] };
  } catch (error) {
    if (error instanceof ApprovalNotGrantedError) return { reads, problems: error.problems };
    if (error instanceof ApprovalRefusedError) return { reads, problems: [error.reason] };
    throw error;
  }
};

const readGolden = (): GoldenCase[] => JSON.parse(readFileSync(FIXTURE, "utf8")) as GoldenCase[];

const sdkResult = (golden: Omit<GoldenCase, "expected">) =>
  golden.kind === "filing"
    ? sdkFiling((golden as FilingCase).input)
    : sdkCheck((golden as CheckCase).input);

const native = rustEngineAvailable() ? describe : describe.skip;

native("@ailu-ai/graph-sdk — catalog approval golden cases (ADR 0045 D3.1)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("the SDK files and checks approvals as recorded", async () => {
    expect(readGolden().length).toBeGreaterThanOrEqual(30);
    for (const golden of readGolden()) {
      const got = await sdkResult(golden);
      vi.restoreAllMocks();
      expect({ name: golden.name, ...got }).toEqual({ name: golden.name, ...golden.expected });
    }
  });

  it("refuses a resume past a request approved by its own requester (new in 2.3.0)", async () => {
    const checked = await sdkCheck({
      graph: {
        id: "gov",
        version: "1",
        name: "gov",
        channels: {},
        nodes: [{ id: "review", type: "human-gate", label: "review" }],
        edges: [],
        entryNodeId: "review"
      },
      state: {
        runId: "run-1",
        graphId: "gov",
        currentNodeId: "review",
        status: "suspended",
        channels: { __approvalIds: ["a-1"] },
        version: 1,
        createdAt: "0",
        updatedAt: "0"
      },
      approvals: {
        "a-1": {
          id: "a-1",
          status: "approved",
          subject: { description: "gate:review" },
          requestedBy: "review",
          resolvedBy: "review"
        }
      }
    });
    expect(checked).toEqual({
      reads: ["a-1"],
      problems: ["request a-1 (gate:review) was approved by its own requester"]
    });
  });

  it("the engine's decisions are the recorded ones", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, ...engineResult(golden) }).toEqual({
        name: golden.name,
        ...golden.expected
      });
    }
  });
});
