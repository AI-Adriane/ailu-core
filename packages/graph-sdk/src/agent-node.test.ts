import { describe, expect, it, vi } from "vitest";

import {
  createGraph,
  DefaultLLMGateway,
  GovernanceMiddlewareRejectedError,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  streamAgentTokens,
  toRustAgentConfig,
  type AgentProfile,
  type EfficiencyMiddlewareSpec,
  type LLMGateway,
  type Message,
  type ToolId
} from "./index.js";

// NOTE: the SDK runs **exclusively on the Rust engine** (the TS fallback was removed —
// see `RustEngineRequiredError`). Agent nodes execute natively on Rust, which builds its
// own gateway from `provider`/`model` + env; a TS `AgentNodeConfig.llm` is not consulted
// on the run path. So this file no longer pins `AILU_SDK_ENGINE=ts` to assert exact
// TS-gateway output text. The agent's *structural* governance contract — runs to
// completion, suspends on an approval-gated tool, resumes via `approveAndResume`,
// routes conditional edges — is covered on the Rust engine by `rust-engine.test.ts`.

const passthrough = { parse: (value: unknown) => value };

describe("@ailu/graph-sdk agent node — suspend on approval (channel-based)", () => {
  const toolCallGateway = (toolName: string): LLMGateway => {
    const gateway = new DefaultLLMGateway();
    gateway.registerAdapter(
      new MockLLMProviderAdapter({
        provider: "anthropic",
        response: {
          content: "",
          toolCalls: [{ id: "tu1", name: toolName, input: {} }],
          stopReason: "tool_use",
          usage: { promptTokens: 0, completionTokens: 0 },
          model: "mock",
          provider: "anthropic"
        }
      })
    );
    return gateway;
  };

  it("suspends the run for approval, then executes the tool once granted on resume", async () => {
    const handler = vi.fn(async () => ({ ok: true }));
    const tools = new InMemoryToolRegistry();
    tools.register(
      {
        id: "refund" as ToolId,
        name: "refund",
        description: "Issues a refund.",
        inputSchema: passthrough,
        outputSchema: passthrough,
        permissions: ["payments:write"],
        requiresApproval: true,
        jsonSchema: { type: "object" }
      },
      handler
    );

    const app = createGraph({ name: "native-approval" })
      .agentNode("assistant", {
        llm: toolCallGateway("refund"),
        prompt: { system: "Use tools when needed." },
        tools,
        suspendForApproval: true,
        maxIterations: 2
      })
      .compile();

    const suspended = await app.run({}, { runId: "run_appr_1" as never });
    expect(suspended.status).toBe("suspended");
    expect(handler).not.toHaveBeenCalled(); // gated before execution

    const done = await app.approveAndResume(suspended.runId, { approvedTools: ["refund"] });
    expect(done.status).toBe("completed");
    expect(handler).toHaveBeenCalled(); // ran once approval was granted
  });
});

describe("@ailu/graph-sdk tool node", () => {
  it("executes the tool calls emitted by the last AI message", async () => {
    const tools = new InMemoryToolRegistry();
    tools.register(
      {
        id: "echo" as ToolId,
        name: "echo",
        description: "Echoes its input.",
        inputSchema: passthrough,
        outputSchema: passthrough,
        permissions: []
      },
      async (input) => ({ echoed: input })
    );

    const app = createGraph({ name: "tool-graph" }).toolNode("tools", { tools }).compile();

    const aiMessage: Message = {
      id: "m1" as Message["id"],
      role: "ai",
      content: "",
      toolCalls: [{ id: "call_1", name: "echo", input: { hello: "world" } }],
      createdAt: new Date()
    };

    const result = await app.run({ messages: [aiMessage] });
    const messages = (result.channels as Record<string, unknown[]>).messages ?? [];

    expect(result.status).toBe("completed");
    expect(messages.length).toBe(2); // original AI message + tool result
    const toolMessage = messages.at(-1) as { role: string; content: string };
    expect(toolMessage.role).toBe("tool");
    expect(toolMessage.content).toContain("world");
  });
});

describe("@ailu/graph-sdk agent node — writeTodos durable channel (ADR 0022/0023)", () => {
  it("threads todosChannel through toRustAgentConfig to the Rust agent spec", () => {
    const config = toRustAgentConfig("planner", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Plan, then act." },
      todosChannel: "__todos"
    });
    expect(config.todosChannel).toBe("__todos");
  });

  it("defaults todosChannel to undefined (no durable sink) when omitted", () => {
    const config = toRustAgentConfig("plain", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Just answer." }
    });
    expect(config.todosChannel).toBeUndefined();
  });

  it("threads enableFs through toRustAgentConfig (ADR 0024 phase 2b)", () => {
    const on = toRustAgentConfig("worker", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Use the filesystem." },
      enableFs: true
    });
    expect(on.enableFs).toBe(true);
    const off = toRustAgentConfig("plain", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "No fs." }
    });
    expect(off.enableFs).toBeUndefined();
  });

  it("threads skills through toRustAgentConfig to the Rust agent spec (ADR 0035 phase 12)", () => {
    const config = toRustAgentConfig("worker", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Draft a reply." },
      skills: { namespace: "skill:t1:org", required: ["house-style@1.0.0"], advisoryK: 2 }
    });
    expect(config.skills).toEqual({
      namespace: "skill:t1:org",
      required: ["house-style@1.0.0"],
      advisoryK: 2
    });
    const off = toRustAgentConfig("plain", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "No skills." }
    });
    expect(off.skills).toBeUndefined();
  });

  it("carries skills on the persisted metadata.agent carrier (ADR 0035 phase 12)", () => {
    // Without this the catalog/Studio path drops the overlay and skills never load.
    const compiled = createGraph({ name: "skill-carrier" })
      .agentNode("worker", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Draft a reply." },
        skills: { namespace: "skill:t1:org", required: ["house-style@1.0.0"], advisoryK: 2 }
      })
      .compile();
    const node = compiled.definition.nodes.find((candidate) => String(candidate.id) === "worker");
    const agent = (node?.metadata as { agent?: Record<string, unknown> } | undefined)?.agent;
    expect(agent?.skills).toEqual({
      namespace: "skill:t1:org",
      required: ["house-style@1.0.0"],
      advisoryK: 2
    });
  });

  it("carries todosChannel + ADR 0014 knobs on the persisted metadata.agent carrier", () => {
    // The persisted GraphDefinition must run identically on the catalog/Studio path,
    // so the metadata.agent carrier has to include these (otherwise they are dropped).
    const compiled = createGraph({ name: "carrier" })
      .agentNode("planner", {
        llm: new DefaultLLMGateway(),
        prompt: { system: "Plan, then act." },
        todosChannel: "__todos",
        outputStyle: "terse",
        contextBudget: 2000,
        enableFs: true
      })
      .compile();
    const node = compiled.definition.nodes.find((candidate) => String(candidate.id) === "planner");
    const agent = (node?.metadata as { agent?: Record<string, unknown> } | undefined)?.agent;
    expect(agent?.todosChannel).toBe("__todos");
    expect(agent?.outputStyle).toBe("terse");
    expect(agent?.contextBudget).toBe(2000);
    expect(agent?.enableFs).toBe(true);
  });
});

describe("@ailu/graph-sdk agent node — profiles + middleware (ADR 0025 phase 3d)", () => {
  const resolved = (config: Partial<Parameters<typeof toRustAgentConfig>[1]>) =>
    toRustAgentConfig("a", {
      llm: new DefaultLLMGateway(),
      prompt: { system: "s" },
      ...config
    });
  const kinds = (mw: EfficiencyMiddlewareSpec[] | undefined) => (mw ?? []).map((m) => m.kind);
  const budget = (mw: EfficiencyMiddlewareSpec[] | undefined): number | undefined => {
    const entry = (mw ?? []).find((m) => m.kind === "contextBudget");
    return entry?.kind === "contextBudget" ? entry.params.chars : undefined;
  };

  it("expands the `fast` profile (tier + full efficiency, no suspend)", () => {
    const config = resolved({ profile: "fast" });
    expect(config.tier).toBe("fast");
    expect(config.suspendForApproval).toBe(false);
    expect(kinds(config.resolvedMiddleware).sort()).toEqual(["compress", "contextBudget", "terse"]);
    expect(budget(config.resolvedMiddleware)).toBe(4000);
  });

  it("expands `frontier-careful` (frontier tier, suspend, NO compression, reflection)", () => {
    const config = resolved({ profile: "frontier-careful" });
    expect(config.tier).toBe("frontier");
    expect(config.suspendForApproval).toBe(true);
    expect(kinds(config.resolvedMiddleware).sort()).toEqual(["contextBudget", "reflection"]);
    expect(budget(config.resolvedMiddleware)).toBe(16000);
  });

  it("expands `governed-deep` (balanced tier, suspend, fs enabled, full efficiency + reflection)", () => {
    const config = resolved({ profile: "governed-deep" });
    expect(config.tier).toBe("balanced");
    expect(config.suspendForApproval).toBe(true);
    expect(config.enableFs).toBe(true);
    expect(kinds(config.resolvedMiddleware).sort()).toEqual([
      "compress",
      "contextBudget",
      "reflection",
      "terse"
    ]);
    expect(budget(config.resolvedMiddleware)).toBe(12000);
  });

  it("lets an explicit field win over the profile default", () => {
    // Explicit tier overrides the profile's; explicit middleware overrides the profile's
    // same-kind entry (dedup, last-writer-wins).
    const config = resolved({
      profile: "fast",
      tier: "frontier",
      middleware: [{ kind: "contextBudget", params: { chars: 9999 } }]
    });
    expect(config.tier).toBe("frontier");
    expect(budget(config.resolvedMiddleware)).toBe(9999);
  });

  it("lets an explicit suspendForApproval:false override a profile that mandates suspend", () => {
    // governed-deep mandates suspend, but an explicit `false` wins (shared resolution, so the
    // TS handler and the Rust/persisted path agree — no human-gate divergence).
    expect(resolved({ profile: "governed-deep", suspendForApproval: false }).suspendForApproval).toBe(false);
    // …and the profile default applies when left unset.
    expect(resolved({ profile: "governed-deep" }).suspendForApproval).toBe(true);
  });

  it("desugars the flat outputStyle/contextBudget knobs into the resolved list", () => {
    const config = resolved({ outputStyle: "terse", contextBudget: 2000 });
    expect(kinds(config.resolvedMiddleware).sort()).toEqual(["contextBudget", "terse"]);
    expect(budget(config.resolvedMiddleware)).toBe(2000);
  });

  it("rejects a governance middleware kind on the builder path", () => {
    expect(() =>
      resolved({
        // A JS caller could smuggle a governance kind past the type — the runtime gate rejects it.
        middleware: [{ kind: "redact" }] as unknown as EfficiencyMiddlewareSpec[]
      })
    ).toThrow(GovernanceMiddlewareRejectedError);
  });

  it("carries resolvedMiddleware on the persisted metadata.agent carrier", () => {
    const profile: AgentProfile = "governed-deep";
    const compiled = createGraph({ name: "carrier" })
      .agentNode("planner", { llm: new DefaultLLMGateway(), prompt: { system: "Plan." }, profile })
      .compile();
    const node = compiled.definition.nodes.find((candidate) => String(candidate.id) === "planner");
    const agent = (node?.metadata as { agent?: { resolvedMiddleware?: EfficiencyMiddlewareSpec[] } } | undefined)
      ?.agent;
    expect(kinds(agent?.resolvedMiddleware).sort()).toEqual([
      "compress",
      "contextBudget",
      "reflection",
      "terse"
    ]);
  });
});

describe("@ailu/graph-sdk streamAgentTokens", () => {
  it("streams the agent's reply token by token via the gateway stream", async () => {
    const gateway = new DefaultLLMGateway();
    gateway.registerAdapter(
      new MockLLMProviderAdapter({
        provider: "anthropic",
        chunks: [
          { delta: "Bon", done: false },
          { delta: "jour", done: false },
          { delta: " !", done: false },
          { delta: "", done: true }
        ]
      })
    );

    const deltas: string[] = [];
    for await (const delta of streamAgentTokens({ llm: gateway, prompt: { system: "Sois bref." } }, "Salut")) {
      deltas.push(delta);
    }

    expect(deltas).toEqual(["Bon", "jour", " !"]); // empty terminal delta is dropped
    expect(deltas.join("")).toBe("Bonjour !");
  });
});
