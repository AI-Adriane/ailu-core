import { describe, expect, it, vi } from "vitest";

import type { GraphDefinition, GraphState } from "@ailu-ai/graph-core";

import { ApprovalRefusedError } from "./errors.js";
import { engineApprovalPlan } from "./rust-engine.js";
import { replayedDecisions } from "./replayed-decisions.js";

vi.mock("./rust-engine.js", () => ({ engineApprovalPlan: vi.fn() }));

const graph = {
  id: "g",
  version: "1",
  name: "g",
  nodes: [],
  edges: []
} as unknown as GraphDefinition;
const state = { runId: "run-1", status: "suspended", channels: {} } as unknown as GraphState;

describe("replayedDecisions", () => {
  it("throws when the engine refuses the wait the replay stopped at, instead of reading no decision", () => {
    vi.mocked(engineApprovalPlan).mockReturnValue({
      clearApprovalIds: true,
      requests: [],
      refusal: "more than 256 approval requests"
    });
    expect(() => replayedDecisions({ status: "suspended", state }, graph)).toThrow(
      ApprovalRefusedError
    );
  });

  it("reads the plan's requests when there is no refusal", () => {
    vi.mocked(engineApprovalPlan).mockReturnValue({
      clearApprovalIds: true,
      requests: [{ subject: { description: "gate:review" } }]
    } as never);
    expect(replayedDecisions({ status: "suspended", state }, graph)).toEqual([
      { status: "", subject: "gate:review" }
    ]);
  });
});
