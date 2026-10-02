import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  explainRun,
  rustEngineAvailable,
  verifyReplayDecisions,
  type GraphState,
  type ReplayDecision,
  type RunEvent
} from "./index.js";

/**
 * ADR 0045 D3.4 — golden cases for what a host reads about a run: `explainRun(state, events?)` and
 * `verifyReplayDecisions(attested, replayed)`, recorded from the TypeScript SDK (its results as
 * JSON). The engine's `explain_run` / `verify_replay_decisions` must give the same, through
 * `@ailu-ai/napi` here, and in the Rust and Python tests that read the same file.
 * `AILU_UPDATE_INSIGHT_GOLDEN=1` rewrites it from the SDK.
 */
const FIXTURE = fileURLToPath(
  new URL("../../../crates/runtime-bridge/tests/fixtures/run_insight_golden.json", import.meta.url)
);

type ExplainCase = {
  name: string;
  kind: "explain";
  input: { state: unknown; events?: unknown[] };
  expected: unknown;
};
type VerifyCase = {
  name: string;
  kind: "verify";
  input: { attested: unknown[]; replayed: unknown[] };
  expected: unknown;
};
type GoldenCase = ExplainCase | VerifyCase;

const napi = (): {
  engineExplainRun: (stateJson: string, eventsJson?: string | null) => string;
  engineVerifyReplayDecisions: (attestedJson: string, replayedJson: string) => string;
} => createRequire(import.meta.url)("@ailu-ai/napi");

/** The SDK's answer for a case, as JSON. */
const sdkResult = (golden: Omit<GoldenCase, "expected">): unknown =>
  JSON.parse(
    JSON.stringify(
      golden.kind === "explain"
        ? explainRun(
            (golden as ExplainCase).input.state as GraphState,
            (golden as ExplainCase).input.events as RunEvent[] | undefined
          )
        : verifyReplayDecisions(
            (golden as VerifyCase).input.attested as ReplayDecision[],
            (golden as VerifyCase).input.replayed as ReplayDecision[]
          )
    )
  );

/** The engine's answer for a case. */
const engineResult = (golden: GoldenCase): unknown =>
  golden.kind === "explain"
    ? JSON.parse(
        napi().engineExplainRun(
          JSON.stringify(golden.input.state),
          golden.input.events === undefined ? undefined : JSON.stringify(golden.input.events)
        )
      )
    : JSON.parse(
        napi().engineVerifyReplayDecisions(
          JSON.stringify(golden.input.attested),
          JSON.stringify(golden.input.replayed)
        )
      );

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

const state = (status: string, currentNodeId: string, channels: Record<string, unknown> = {}) => ({
  runId: "run-1",
  graphId: "g",
  currentNodeId,
  status,
  channels,
  version: 4,
  checkpointId: "run-1:4",
  createdAt: "0",
  updatedAt: "0"
});
const event = (type: string, extra: Record<string, unknown> = {}) => ({
  type,
  runId: "run-1",
  timestamp: "0",
  ...extra
});
const asks = (...subjects: unknown[]) => ({
  approvalRequests: subjects.map((subject) => ({ subject, reason: "needs approval" }))
});
const explain = (name: string, input: ExplainCase["input"]): Omit<ExplainCase, "expected"> => ({
  name,
  kind: "explain",
  input
});
const verify = (
  name: string,
  attested: unknown[],
  replayed: unknown[]
): Omit<VerifyCase, "expected"> => ({ name, kind: "verify", input: { attested, replayed } });
const decision = (status: string, subject: string, extra: Record<string, unknown> = {}) => ({
  status,
  subject,
  ...extra
});

const caseSources = (): Omit<GoldenCase, "expected">[] => [
  explain("suspended at a human gate, internal channels hidden", {
    state: state("suspended", "review", {
      draft: "x",
      __approvalIds: ["a-1"],
      __suspend: { reason: "human-gate" },
      __signals: {},
      __approvedTools: []
    })
  }),
  explain("suspended without a reason: an interrupt", {
    state: state("suspended", "agent", { a: 1 })
  }),
  explain("a durable timer", {
    state: state("suspended", "sleep", {
      __suspend: { reason: "timer", wakeAt: "2026-10-03T08:00:00Z" }
    })
  }),
  explain("a signal wait with a timeout: the signal wins", {
    state: state("suspended", "wait", {
      __suspend: { reason: "signal", awaitingSignal: "paid", wakeAt: "2026-10-03T08:00:00Z" }
    })
  }),
  explain("tools waiting for approval, sorted, once each, strings only", {
    state: state("suspended", "assistant", {
      __suspend: { reason: "interrupt" },
      agentResult: asks(
        "tool:wire",
        "tool:refund",
        "tool:refund",
        "gate:x",
        { description: "tool:y" },
        5
      ),
      other: asks("tool:alpha"),
      text: "tool:not-a-request",
      list: [asks("tool:in-a-list")],
      nothing: null
    })
  }),
  explain("a suspend reason that is not a string reads as an interrupt", {
    state: state("suspended", "n", { __suspend: { reason: 5, wakeAt: "later" } })
  }),
  explain("tools pending under another reason", {
    state: state("suspended", "n", { __suspend: { reason: "custom" }, out: asks("tool:refund") })
  }),
  explain("another reason, nothing pending", {
    state: state("suspended", "n", { __suspend: { reason: "custom" } })
  }),
  explain("failed: the last failure is the run's", {
    state: state("failed", "send"),
    events: [
      event("node_started", { nodeId: "send" }),
      event("node_failed", { nodeId: "send", error: "503", attempt: 1, category: "transient" }),
      event("run_failed", { error: "send failed after retries" })
    ]
  }),
  explain("failed: the last failure is a node's", {
    state: state("failed", "send"),
    events: [
      event("run_failed", { error: "first" }),
      event("node_failed", { nodeId: "send", error: "boom", attempt: 2 })
    ]
  }),
  explain("failed at a node with an empty id", {
    state: state("failed", "send"),
    events: [event("node_failed", { nodeId: "", error: "boom", attempt: 1 })]
  }),
  explain("failed without events", { state: state("failed", "send", { a: 1 }) }),
  explain("failed, events without a failure", {
    state: state("failed", "send"),
    events: [
      event("node_started", { nodeId: "send" }),
      event("node_completed", { nodeId: "send", output: {} })
    ]
  }),
  explain("completed with channels", {
    state: state("completed", "end", { b: 2, a: 1, __todos: [] })
  }),
  explain("completed with no public channel", {
    state: state("completed", "end", { __approvalIds: [] })
  }),
  explain("running", { state: state("running", "step") }),
  explain("cancelled", { state: state("cancelled", "step2", { work: null }) }),
  explain("more than twenty events, some without a node", {
    state: state("completed", "end", { out: 1 }),
    events: [
      ...Array.from({ length: 22 }, (_, n) => event("node_started", { nodeId: `n${n}` })),
      event("run_completed", { finalState: {} }),
      event("token_delta", { nodeId: "chat", messageId: "m", delta: "hi" })
    ]
  }),
  explain("no events: an empty list", { state: state("running", "step"), events: [] }),
  explain("channel names sort by UTF-16 code unit", {
    state: state("completed", "end", { "": 1, "😀": 2, é: 3, Z: 4, a: 5 })
  }),
  verify(
    "every decision reproduced",
    [decision("approved", "gate:review"), decision("rejected", "tool:refund")],
    [decision("approved", "gate:review"), decision("rejected", "tool:refund")]
  ),
  verify(
    "a decision the replay did not make",
    [decision("approved", "gate:a"), decision("approved", "gate:b")],
    [decision("approved", "gate:a")]
  ),
  verify(
    "a decision the run did not attest",
    [decision("approved", "gate:a")],
    [decision("approved", "gate:a"), decision("approved", "tool:extra")]
  ),
  verify(
    "a flipped status and another subject",
    [decision("approved", "gate:a"), decision("approved", "tool:x")],
    [decision("rejected", "gate:a"), decision("approved", "tool:y")]
  ),
  verify("no decisions", [], []),
  verify(
    "other fields are carried, not compared",
    [decision("approved", "gate:a", { decidedAt: "1", resolvedBy: "alice", approvalId: "x" })],
    [decision("approved", "gate:a", { decidedAt: "2" })]
  )
];

const readGolden = (): GoldenCase[] => JSON.parse(readFileSync(FIXTURE, "utf8")) as GoldenCase[];

const native = rustEngineAvailable() ? describe : describe.skip;

native("@ailu-ai/graph-sdk — run insight golden cases (ADR 0045 D3.4)", () => {
  if (process.env.AILU_UPDATE_INSIGHT_GOLDEN === "1") {
    it("rewrites the golden file from the TypeScript SDK", () => {
      const cases = caseSources().map((source) => {
        const input = JSON.parse(JSON.stringify(source.input)) as GoldenCase["input"];
        return { ...source, input, expected: sdkResult({ ...source, input } as GoldenCase) };
      });
      writeFileSync(FIXTURE, `${JSON.stringify(cases, null, 2)}\n`);
    });
    return;
  }

  it("covers every case the file lists", () => {
    expect(readGolden().map((golden) => golden.name)).toEqual(
      caseSources().map((source) => source.name)
    );
  });

  it("the SDK explains runs and checks replays as recorded", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, got: sdkResult(golden) }).toEqual({
        name: golden.name,
        got: golden.expected
      });
    }
  });

  it("the engine explains runs and checks replays as recorded", () => {
    for (const golden of readGolden()) {
      expect({ name: golden.name, got: engineResult(golden) }).toEqual({
        name: golden.name,
        got: golden.expected
      });
    }
  });
});
