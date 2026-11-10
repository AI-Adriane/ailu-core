import { describe, expect, it, vi } from "vitest";
import type { GraphState, RunId } from "@ailu-ai/graph-core";
import { TextModelAccessUnsupportedError } from "@ailu-ai/model-core";
import { RustGraphRunner, type RustRunnerParts } from "./rust-engine.js";
import type { ChannelValues } from "./typed.js";

const oldRunner = (policy?: unknown) => {
  const call = vi.fn(async () =>
    JSON.stringify({
      status: "completed",
      state: { runId: "r", status: "completed", channels: {} }
    })
  );
  const native = {
    engineRun: call,
    engineResume: call,
    engineApproveAndResume: call,
    engineSignal: call,
    engineReplay: call
  };
  const parts: RustRunnerParts<ChannelValues> = {
    textModelAccess: policy as RustRunnerParts<ChannelValues>["textModelAccess"],
    definition: {
      id: "g",
      version: "1",
      name: "g",
      channels: {},
      nodes: [],
      edges: [],
      entryNodeId: "n"
    } as unknown as RustRunnerParts<ChannelValues>["definition"],
    subgraphs: [],
    nodeFns: new Map(),
    toolFns: new Map(),
    conditions: new Map(),
    agents: new Map(),
    components: new Map(),
    mapAgents: new Map(),
    jsNodeIds: new Set(),
    jsToolNames: new Set()
  };
  return { runner: new RustGraphRunner(native, parts), call };
};

describe("old native addons cannot discard governed resume metadata", () => {
  it.each(["resume", "approve", "signal", "replay"] as const)(
    "refuses %s before native I/O when the state is governed but options were lost",
    async (entry) => {
      const { runner, call } = oldRunner();
      const state = { runId: "r", textModelAccess: { schemaVersion: 1 } } as unknown as GraphState;
      const action =
        entry === "resume"
          ? runner.resume(state)
          : entry === "approve"
            ? runner.approveAndResume(state, [])
            : entry === "signal"
              ? runner.signal(state, "wake", {})
              : runner.replay(state, "cp", "{}");
      await expect(action).rejects.toThrow(TextModelAccessUnsupportedError);
      expect(call).not.toHaveBeenCalled();
    }
  );
  it("does not erase an explicitly null snapshot before checking an older addon", async () => {
    const { runner, call } = oldRunner(null);
    await expect(runner.run("r" as RunId, {})).rejects.toThrow(TextModelAccessUnsupportedError);
    expect(call).not.toHaveBeenCalled();
  });
  it("continues to forward a legacy resume to an older addon", async () => {
    const { runner, call } = oldRunner();
    await runner.resume({ runId: "r", channels: {} } as unknown as GraphState);
    expect(call).toHaveBeenCalledTimes(1);
  });
});
