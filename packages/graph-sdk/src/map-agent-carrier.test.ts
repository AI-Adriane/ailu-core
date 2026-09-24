import { describe, expect, it } from "vitest";

import { readMapAgentCarrier } from "./run-catalog-graph.js";

describe("@ailu-ai/graph-sdk — readMapAgentCarrier", () => {
  it("narrows a valid mapAgents carrier and defaults suspendForApproval to false", () => {
    const carrier = readMapAgentCarrier({
      mapAgents: {
        overChannel: "items",
        joinAt: "results",
        subAgent: { provider: "anthropic", system: "Summarize the item." }
      }
    });
    expect(carrier).toEqual({
      overChannel: "items",
      joinAt: "results",
      subAgent: { provider: "anthropic", system: "Summarize the item." },
      suspendForApproval: false
    });
  });

  it("carries suspendForApproval when set", () => {
    const carrier = readMapAgentCarrier({
      mapAgents: { overChannel: "i", joinAt: "r", subAgent: {}, suspendForApproval: true }
    });
    expect(carrier?.suspendForApproval).toBe(true);
  });

  it("rejects a carrier missing overChannel / joinAt / subAgent", () => {
    expect(readMapAgentCarrier({ mapAgents: { joinAt: "r", subAgent: {} } })).toBeUndefined();
    expect(readMapAgentCarrier({ mapAgents: { overChannel: "i", subAgent: {} } })).toBeUndefined();
    expect(readMapAgentCarrier({ mapAgents: { overChannel: "i", joinAt: "r" } })).toBeUndefined();
  });

  it("returns undefined for a non-mapAgents metadata bag", () => {
    expect(readMapAgentCarrier({ agent: { provider: "anthropic" } })).toBeUndefined();
    expect(readMapAgentCarrier(undefined)).toBeUndefined();
  });
});
