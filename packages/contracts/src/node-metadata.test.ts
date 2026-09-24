import { describe, expect, it } from "vitest";

import {
  AgentNodeMetadataSchema,
  MapAgentNodeMetadataSchema,
  readAgentMetadata,
  readMapAgentMetadata
} from "./node-metadata.js";

/**
 * The agent carrier is the API↔Studio boundary for a persisted agent node. The
 * ADR 0014 (outputStyle/contextBudget) and ADR 0022/0023 (todosChannel) knobs must
 * SURVIVE a parse round-trip — Zod strips unknown keys, so a field missing from the
 * schema would be silently dropped on the catalog/Studio path (the bug this guards).
 */
describe("@ailu-ai/contracts — AgentNodeMetadataSchema", () => {
  it("keeps outputStyle, contextBudget, todosChannel and enableFs through a parse round-trip", () => {
    const carrier = {
      provider: "anthropic",
      toolNames: ["writeTodos"],
      outputChannel: "agentResult",
      outputStyle: "terse" as const,
      contextBudget: 4000,
      todosChannel: "__todos",
      enableFs: true
    };
    const parsed = AgentNodeMetadataSchema.parse(carrier);
    expect(parsed.outputStyle).toBe("terse");
    expect(parsed.contextBudget).toBe(4000);
    expect(parsed.todosChannel).toBe("__todos");
    expect(parsed.enableFs).toBe(true);
  });

  it("reads the carrier off an open metadata bag without dropping the new fields", () => {
    const meta = readAgentMetadata({ agent: { provider: "anthropic", todosChannel: "__todos" } });
    expect(meta?.todosChannel).toBe("__todos");
  });

  it("rejects an outputStyle other than terse", () => {
    expect(AgentNodeMetadataSchema.safeParse({ outputStyle: "verbose" }).success).toBe(false);
  });

  it("leaves the new fields undefined when omitted", () => {
    const parsed = AgentNodeMetadataSchema.parse({ provider: "anthropic" });
    expect(parsed.todosChannel).toBeUndefined();
    expect(parsed.outputStyle).toBeUndefined();
    expect(parsed.contextBudget).toBeUndefined();
    expect(parsed.mcpConnectionId).toBeUndefined();
  });

  // ADR 0075 (issue #566 G3) — the MCP connection carrier.
  it("keeps mcpConnectionId through a parse round-trip", () => {
    const parsed = AgentNodeMetadataSchema.parse({ provider: "anthropic", mcpConnectionId: "conn-1" });
    expect(parsed.mcpConnectionId).toBe("conn-1");
  });

  it("rejects an empty mcpConnectionId", () => {
    expect(AgentNodeMetadataSchema.safeParse({ mcpConnectionId: "" }).success).toBe(false);
  });

  // Issue #566 G19 — governed action-tool connectors, generalized (keyed by provider id).
  it("keeps actionConnections through a parse round-trip", () => {
    const parsed = AgentNodeMetadataSchema.parse({
      provider: "anthropic",
      actionConnections: { slack: "conn-1" }
    });
    expect(parsed.actionConnections).toEqual({ slack: "conn-1" });
  });

  it("rejects an actionConnections entry with an empty value", () => {
    expect(AgentNodeMetadataSchema.safeParse({ actionConnections: { slack: "" } }).success).toBe(false);
  });

  // ADR 0025 phase 3d — the persisted resolvedMiddleware gate.
  it("keeps an efficiency-only resolvedMiddleware list through a parse round-trip", () => {
    const parsed = AgentNodeMetadataSchema.parse({
      provider: "anthropic",
      resolvedMiddleware: [
        { kind: "compress" },
        { kind: "terse" },
        { kind: "contextBudget", params: { chars: 4000 } }
      ]
    });
    expect(parsed.resolvedMiddleware?.map((m) => m.kind)).toEqual([
      "compress",
      "terse",
      "contextBudget"
    ]);
  });

  it("rejects a GOVERNANCE kind in resolvedMiddleware by construction (the persisted reject gate)", () => {
    // A governance kind is not in the efficiency-only discriminated union → safeParse fails →
    // readAgentMetadata drops the whole malformed agent. An ungoverned stack is unrepresentable.
    for (const kind of ["redact", "approvalGate", "fsPolicy"]) {
      expect(AgentNodeMetadataSchema.safeParse({ resolvedMiddleware: [{ kind }] }).success).toBe(
        false
      );
      expect(
        readAgentMetadata({ agent: { provider: "anthropic", resolvedMiddleware: [{ kind }] } })
      ).toBeUndefined();
    }
  });

  it("rejects a contextBudget entry missing its chars param", () => {
    expect(
      AgentNodeMetadataSchema.safeParse({ resolvedMiddleware: [{ kind: "contextBudget" }] }).success
    ).toBe(false);
  });

  // ADR 0025 phase 3e — reflection variant.
  it("accepts a reflection entry (bare and with a threshold), rejects an out-of-range threshold", () => {
    expect(
      AgentNodeMetadataSchema.safeParse({ resolvedMiddleware: [{ kind: "reflection" }] }).success
    ).toBe(true);
    expect(
      AgentNodeMetadataSchema.safeParse({
        resolvedMiddleware: [{ kind: "reflection", params: { threshold: 0.9 } }]
      }).success
    ).toBe(true);
    expect(
      AgentNodeMetadataSchema.safeParse({
        resolvedMiddleware: [{ kind: "reflection", params: { threshold: 1.5 } }]
      }).success
    ).toBe(false);
  });
});

// ADR 0027 phase 4b / ADR 0049 — the mapAgents (dynamic fan-out) carrier.
describe("@ailu-ai/contracts — MapAgentNodeMetadataSchema", () => {
  it("keeps overChannel/joinAt/subAgent through a parse round-trip", () => {
    const parsed = MapAgentNodeMetadataSchema.parse({
      overChannel: "items",
      joinAt: "results",
      subAgent: { provider: "anthropic", system: "Summarize the item.", enableFs: true },
      suspendForApproval: true
    });
    expect(parsed.overChannel).toBe("items");
    expect(parsed.joinAt).toBe("results");
    expect(parsed.subAgent.system).toBe("Summarize the item.");
    expect(parsed.subAgent.enableFs).toBe(true);
    expect(parsed.suspendForApproval).toBe(true);
  });

  it("requires overChannel, joinAt and subAgent", () => {
    expect(MapAgentNodeMetadataSchema.safeParse({ joinAt: "r", subAgent: {} }).success).toBe(false);
    expect(MapAgentNodeMetadataSchema.safeParse({ overChannel: "i", subAgent: {} }).success).toBe(
      false
    );
    expect(MapAgentNodeMetadataSchema.safeParse({ overChannel: "i", joinAt: "r" }).success).toBe(
      false
    );
  });

  it("readMapAgentMetadata narrows the carrier off an open metadata bag", () => {
    const meta = readMapAgentMetadata({
      mapAgents: { overChannel: "items", joinAt: "results", subAgent: { provider: "anthropic" } }
    });
    expect(meta?.overChannel).toBe("items");
    expect(readMapAgentMetadata({ agent: { provider: "anthropic" } })).toBeUndefined();
    expect(readMapAgentMetadata(undefined)).toBeUndefined();
  });
});
