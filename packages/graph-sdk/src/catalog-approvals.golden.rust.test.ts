import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import type { ApprovalEngine, ApprovalId, ApprovalRequest } from "@ailu-ai/approval-engine";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApprovalNotGrantedError,
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
 * Recorded from the TypeScript SDK through its public API, with the engine's run entries stubbed
 * to return each case's state. The engine's own decisions must give the same: the Rust tests of
 * `crates/runtime-bridge` read the same file. `AILU_UPDATE_APPROVALS_GOLDEN=1` rewrites it.
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
  expected: { requests: unknown[]; approvalIds: unknown };
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
    };
    const kept = plan.clearApprovalIds ? [] : ids(golden.input.state);
    return {
      requests: plan.requests,
      approvalIds: plan.requests.length > 0 ? plan.requests.map((_, n) => `filed-${n}`) : kept
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
    const outcome = await runCatalogGraph(input.graph as GraphDefinition, {
      runId: state.runId,
      approvalEngine: engine,
      subgraphs
    });
    return { requests: filed, approvalIds: ids(outcome.state) };
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
    throw error;
  }
};

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

const node = (id: string, type = "action", metadata?: unknown): Record<string, unknown> =>
  metadata === undefined ? { id, type, label: id } : { id, type, label: id, metadata };
const graph = (id: string, nodes: unknown[]): Record<string, unknown> => ({
  id,
  version: "1",
  name: id,
  channels: {},
  nodes,
  edges: [],
  entryNodeId: (nodes[0] as { id: string }).id
});
const state = (
  currentNodeId: string,
  channels: Record<string, unknown>,
  status = "suspended",
  runId = "run-1"
): Record<string, unknown> => ({
  runId,
  graphId: "g",
  currentNodeId,
  status,
  channels,
  version: 3,
  checkpointId: `${runId}:3`,
  createdAt: "0",
  updatedAt: "0"
});
const asks = (...subjects: unknown[]): Record<string, unknown> => ({
  approvalRequests: subjects.map((subject) => ({ subject, reason: "needs approval" }))
});
const agent = (id: string, extra: Record<string, unknown> = {}) =>
  node(id, "agent", {
    agent: { provider: "anthropic", toolNames: ["refund"], suspendForApproval: true, ...extra }
  });
const gate = (id: string) => node(id, "human-gate");
const sub = (id: string, subgraphId: string, extra: Record<string, unknown> = {}) => ({
  ...node(id, "subgraph"),
  subgraphId,
  ...extra
});

const childGraph = graph("child", [agent("c_agent"), gate("c_gate"), node("c_step")]);
const childSuspendedAtAgent = state(
  "c_agent",
  { agentResult: asks("tool:refund") },
  "suspended",
  "run-1:sub"
);
const childSuspendedAtGate = state("c_gate", {}, "suspended", "run-1:sub");
const parentWithChild = graph("parent", [node("p_step"), sub("sub", "child")]);

const filing = (name: string, input: FilingCase["input"]): Omit<FilingCase, "expected"> => ({
  name,
  kind: "filing",
  input
});

const record = (
  status: string,
  description: unknown,
  resolvedBy?: string,
  requestedBy = "assistant"
) => ({
  id: "x",
  runId: "run-1",
  nodeId: requestedBy,
  requestedBy,
  subject: description === undefined ? { kind: "artifact", id: "a-1" } : { description },
  status,
  ...(resolvedBy === undefined ? {} : { resolvedBy }),
  createdAt: "1970-01-01T00:00:00.000Z"
});

const check = (name: string, input: CheckCase["input"]): Omit<CheckCase, "expected"> => ({
  name,
  kind: "check",
  input
});

const governedAgentGraph = graph("gov", [agent("assistant")]);
const twoGates = graph("two-gates", [gate("legal"), gate("finance")]);

const caseSources = (): Omit<GoldenCase, "expected">[] => [
  // -- what a run files ------------------------------------------------------------------------
  filing("an agent's gated tools, string subjects", {
    graph: governedAgentGraph,
    state: state("assistant", { agentResult: asks("tool:refund", "tool:wire") })
  }),
  filing("object subjects, and entries that are not requests", {
    graph: governedAgentGraph,
    state: state("assistant", {
      agentResult: {
        approvalRequests: [
          { subject: { description: "tool:refund", extra: 1 } },
          null,
          7,
          "tool:raw",
          { subject: 5 },
          { subject: { description: 5 } },
          { subject: ["tool:x"] },
          { reason: "no subject" },
          { subject: "tool:wire" }
        ]
      }
    })
  }),
  filing("a custom output channel, and two agents sharing one", {
    graph: graph("g", [
      agent("a1", { outputChannel: "out" }),
      agent("a2", { outputChannel: "out" }),
      agent("a3", { outputChannel: null }),
      agent("a4")
    ]),
    state: state("a1", { out: asks("tool:refund"), agentResult: asks("tool:wire") })
  }),
  filing("a human gate", { graph: twoGates, state: state("legal", {}) }),
  filing("an agent's tools and the gate it stopped at", {
    graph: graph("g", [agent("assistant"), gate("review")]),
    state: state("review", { agentResult: asks("tool:refund") })
  }),
  filing("a completed run files nothing", {
    graph: governedAgentGraph,
    state: state("assistant", { agentResult: asks("tool:refund") }, "completed")
  }),
  filing("a failed run files nothing", { graph: twoGates, state: state("legal", {}, "failed") }),
  filing("ids already stashed: nothing filed again", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["kept-1"] })
  }),
  filing("an empty stash files", { graph: twoGates, state: state("legal", { __approvalIds: [] }) }),
  filing("a stash that is not a list files", {
    graph: twoGates,
    state: state("legal", { __approvalIds: "kept" })
  }),
  filing("a direct child suspended at its agent, deterministic child run id", {
    graph: parentWithChild,
    subgraphs: [childGraph],
    state: state("sub", {
      agentResult: asks("tool:parent"),
      __subgraphStates: { "run-1:sub": childSuspendedAtAgent }
    })
  }),
  filing("a direct child suspended at its gate, run id from __subgraphRuns", {
    graph: parentWithChild,
    subgraphs: [childGraph],
    state: state("sub", {
      __subgraphRuns: { sub: "custom-child" },
      __subgraphStates: { "custom-child": { ...childSuspendedAtGate, runId: "custom-child" } }
    })
  }),
  filing("a child that is not suspended", {
    graph: parentWithChild,
    subgraphs: [childGraph],
    state: state("sub", {
      __subgraphStates: { "run-1:sub": { ...childSuspendedAtGate, status: "completed" } }
    })
  }),
  filing("a mapSubgraph child is not walked, nor one with mapSubgraph: null", {
    graph: graph("parent", [
      sub("sub", "child", { mapSubgraph: { overChannel: "items", joinAt: "out" } }),
      sub("sub2", "child", { mapSubgraph: null })
    ]),
    subgraphs: [childGraph],
    state: state("sub", {
      __subgraphStates: { "run-1:sub": childSuspendedAtGate, "run-1:sub2": childSuspendedAtGate }
    })
  }),
  filing("a child whose definition is not given", {
    graph: parentWithChild,
    subgraphs: [],
    state: state("sub", { __subgraphStates: { "run-1:sub": childSuspendedAtGate } })
  }),
  filing("two definitions with the child's id: the last wins", {
    graph: parentWithChild,
    subgraphs: [graph("child", [gate("first_gate")]), graph("child", [gate("c_gate")])],
    state: state("sub", { __subgraphStates: { "run-1:sub": childSuspendedAtGate } })
  }),
  filing("a child's own subgraph is not walked", {
    graph: parentWithChild,
    subgraphs: [
      graph("child", [sub("inner", "grandchild"), gate("c_gate")]),
      graph("grandchild", [gate("g_gate")])
    ],
    state: state("sub", {
      __subgraphStates: {
        "run-1:sub": {
          ...state("inner", { __subgraphStates: { "run-1:sub:inner": state("g_gate", {}) } }),
          runId: "run-1:sub"
        }
      }
    })
  }),
  filing("a child snapshot without channels, or that is not an object", {
    graph: graph("parent", [sub("sub", "child"), sub("sub2", "child")]),
    subgraphs: [childGraph],
    state: state("sub", {
      __subgraphStates: {
        "run-1:sub": { status: "suspended", currentNodeId: "c_gate" },
        "run-1:sub2": "x"
      }
    })
  }),
  filing("an agent carrier on a component node is still read", {
    graph: graph("g", [
      node("both", "action", {
        component: { kind: "textCleaner", params: {} },
        agent: { outputChannel: "both_out" }
      })
    ]),
    state: state("both", { both_out: asks("tool:refund") })
  }),
  filing("output channels that are a list, a string or null", {
    graph: graph("g", [
      agent("a1", { outputChannel: "list" }),
      agent("a2", { outputChannel: "text" }),
      agent("a3", { outputChannel: "nothing" })
    ]),
    state: state("a1", { list: [asks("tool:x")], text: "tool:y", nothing: null })
  }),
  filing("the current node is not a gate", {
    graph: graph("g", [node("step"), gate("review")]),
    state: state("step", {})
  }),
  // -- what a resume files ---------------------------------------------------------------------
  filing("resumed to the same gate: the stash is kept, nothing filed", {
    graph: governedAgentGraph,
    previousState: state("assistant", {
      agentResult: asks("tool:refund"),
      __approvalIds: ["kept-1"]
    }),
    state: state("assistant", { agentResult: asks("tool:refund"), __approvalIds: ["kept-1"] })
  }),
  filing("resumed to the next gate: the stash is cleared and the new gate filed", {
    graph: twoGates,
    previousState: state("legal", { __approvalIds: ["legal-1"] }),
    state: state("finance", { __approvalIds: ["legal-1"] })
  }),
  filing("resumed to completion: the stash is cleared", {
    graph: twoGates,
    previousState: state("legal", { __approvalIds: ["legal-1"] }),
    state: state("finance", { __approvalIds: ["legal-1"] }, "completed")
  }),
  filing("resumed to completion without a stash channel: it is written empty", {
    graph: graph("g", [node("wait"), gate("review")]),
    previousState: state("wait", {}),
    state: state("review", {}, "completed")
  }),
  filing("resumed from a run that was not suspended, to a gate", {
    graph: twoGates,
    previousState: state("legal", {}, "running"),
    state: state("finance", {})
  }),
  filing("resumed at the same node, asking for another tool", {
    graph: governedAgentGraph,
    previousState: state("assistant", {
      agentResult: asks("tool:refund"),
      __approvalIds: ["kept-1"]
    }),
    state: state("assistant", { agentResult: asks("tool:wire"), __approvalIds: ["kept-1"] })
  }),
  filing("resumed at the same node, same tools in another order", {
    graph: governedAgentGraph,
    previousState: state("assistant", {
      agentResult: asks("tool:refund", "tool:wire"),
      __approvalIds: ["kept-1", "kept-2"]
    }),
    state: state("assistant", {
      agentResult: asks("tool:wire", "tool:refund"),
      __approvalIds: ["kept-1", "kept-2"]
    })
  }),
  // -- whether a resume may go on --------------------------------------------------------------
  check("a run that is not suspended is not checked", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["a-1"] }, "running"),
    approvals: {}
  }),
  check("nothing stashed and nothing to decide", {
    graph: graph("g", [node("step")]),
    state: state("step", {}),
    approvals: {}
  }),
  check("nothing stashed but a gate waits: never recorded", {
    graph: twoGates,
    state: state("legal", {}),
    approvals: {}
  }),
  check("nothing stashed but a child's tool waits: never recorded", {
    graph: parentWithChild,
    subgraphs: [childGraph],
    state: state("sub", { __subgraphStates: { "run-1:sub": childSuspendedAtAgent } }),
    approvals: {}
  }),
  check("a pending request", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["a-1"] }),
    approvals: { "a-1": record("pending", "gate:legal", undefined, "legal") }
  }),
  check("a rejected gate, and one rejected by no one on record", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["a-1", "a-2"] }),
    approvals: {
      "a-1": record("rejected", "gate:legal", "alice", "legal"),
      "a-2": record("rejected", "gate:legal", undefined, "legal")
    }
  }),
  check("a rejected tool does not block", {
    graph: governedAgentGraph,
    state: state("assistant", { __approvalIds: ["a-1"] }),
    approvals: { "a-1": record("rejected", "tool:refund", "alice") }
  }),
  check("an approved tool, granted by its approver", {
    graph: governedAgentGraph,
    state: state("assistant", { __approvalIds: ["a-1"] }),
    approvedTools: [{ name: "refund", requestedBy: "assistant", resolvedBy: "alice" }],
    approvals: { "a-1": record("approved", "tool:refund", "alice") }
  }),
  check("a grant naming another approver, and a grant with no request", {
    graph: governedAgentGraph,
    state: state("assistant", { __approvalIds: ["a-1"] }),
    approvedTools: [
      { name: "refund", requestedBy: "assistant", resolvedBy: "mallory" },
      { name: "wire", requestedBy: "assistant", resolvedBy: "alice" }
    ],
    approvals: { "a-1": record("approved", "tool:refund", "alice") }
  }),
  check("an id the engine does not know", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["a-1", "ghost"] }),
    approvals: { "a-1": record("approved", "gate:legal", "alice", "legal"), ghost: null }
  }),
  check("mixed: approved gate, pending tool, unknown, ids that are not strings, duplicates", {
    graph: graph("g", [agent("assistant"), gate("review")]),
    state: state("review", { __approvalIds: ["a-1", 42, "a-1", "a-3", true] }),
    approvedTools: [{ name: "refund", requestedBy: "assistant", resolvedBy: "alice" }],
    approvals: {
      "a-1": record("approved", "gate:review", "alice", "review"),
      "42": record("pending", "tool:refund"),
      "a-3": record("approved", "tool:refund", "alice"),
      true: record("rejected", "tool:wire", "bob")
    }
  }),
  check("a subject that is not a description reads as empty", {
    graph: governedAgentGraph,
    state: state("assistant", { __approvalIds: ["a-1", "a-2"] }),
    approvals: { "a-1": record("pending", undefined), "a-2": record("rejected", undefined) }
  }),
  check("an approved gate lets the resume go on", {
    graph: twoGates,
    state: state("legal", { __approvalIds: ["a-1"] }),
    approvals: { "a-1": record("approved", "gate:legal", "alice", "legal") }
  })
];

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

  if (process.env.AILU_UPDATE_APPROVALS_GOLDEN === "1") {
    it("rewrites the golden file from the TypeScript SDK", async () => {
      const cases: GoldenCase[] = [];
      for (const source of caseSources()) {
        const input = JSON.parse(JSON.stringify(source.input)) as GoldenCase["input"];
        const expected = await sdkResult({ ...source, input } as GoldenCase);
        vi.restoreAllMocks();
        cases.push({ ...source, input, expected } as GoldenCase);
      }
      writeFileSync(FIXTURE, `${JSON.stringify(cases, null, 2)}\n`);
    });
    return;
  }

  it("covers every case the file lists", () => {
    expect(readGolden().map((golden) => golden.name)).toEqual(
      caseSources().map((source) => source.name)
    );
  });

  it("the SDK files and checks approvals as recorded", async () => {
    for (const golden of readGolden()) {
      const got = await sdkResult(golden);
      vi.restoreAllMocks();
      expect({ name: golden.name, ...got }).toEqual({ name: golden.name, ...golden.expected });
    }
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
