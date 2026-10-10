import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  ApprovalScopeUnsupportedError,
  createGraph,
  InMemoryToolRegistry,
  model,
  rustEngineAvailable,
  type AgentResult
} from "./index.js";

/**
 * ADR 0051 D4/D5 on the Rust engine, end to end: an agent with `approvalScope: "call"` is granted
 * one call per signature. A local OpenAI-compatible server plays a model that refunds two orders,
 * one after the other, then answers: the first refund waits for a signature on THAT call, a name
 * grant unlocks nothing, and the second refund — other arguments — waits for its own signature.
 * The default scope keeps one name grant for both refunds.
 */

const readBody = (request: IncomingMessage): Promise<string> =>
  new Promise((resolve, reject) => {
    let body = "";
    request.on("data", (chunk: Buffer) => {
      body += chunk.toString("utf8");
    });
    request.on("end", () => resolve(body));
    request.on("error", reject);
  });

const usage = { prompt_tokens: 1, completion_tokens: 1 };

const REFUNDS = [
  { order: "A-1", amount: 40 },
  { order: "A-2", amount: 900 }
];

const refundCall = (index: number) => ({
  model: "llama-3",
  choices: [
    {
      message: {
        role: "assistant",
        content: null,
        tool_calls: [
          {
            id: `call_${index}`,
            type: "function",
            function: { name: "refund", arguments: JSON.stringify(REFUNDS[index]) }
          }
        ]
      },
      finish_reason: "tool_calls"
    }
  ],
  usage
});

const answer = {
  model: "llama-3",
  choices: [{ message: { role: "assistant", content: "FINAL: refunded" }, finish_reason: "stop" }],
  usage
};

const refundTools = (calls: unknown[]): InMemoryToolRegistry => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "refund" as never,
      name: "refund",
      description: "Refund an order.",
      inputSchema: { parse: (value: unknown) => value as Record<string, unknown> },
      outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
      permissions: [],
      requiresApproval: true,
      jsonSchema: {
        type: "object",
        properties: { order: { type: "string" }, amount: { type: "number" } },
        required: ["order", "amount"]
      }
    },
    async (input) => {
      calls.push(input);
      return { ok: true };
    }
  );
  return tools;
};

type FiledCall = { subject: unknown; approvalKey?: string; callKey?: string; input?: unknown };

const filedOf = (channels: unknown): FiledCall | undefined =>
  (channels as Record<string, AgentResult | undefined>).agentResult?.approvalRequests[0] as
    FiledCall | undefined;

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust('approvalScope: "call" (ADR 0051 D4/D5) on the Rust engine', () => {
  let server: Server;
  let baseURL = "";

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((raw) => {
        const body = JSON.parse(raw) as { messages?: Array<{ role?: string }> };
        // The agent restarts its conversation on each run of the node: count this attempt's results.
        const results = (body.messages ?? []).filter((message) => message.role === "tool").length;
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(results < REFUNDS.length ? refundCall(results) : answer));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    baseURL = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  const appWith = (calls: unknown[], approvalScope?: "tool" | "call") =>
    createGraph({ name: "approval-per-call" })
      .agentNode("assistant", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        tools: refundTools(calls),
        prompt: { system: "Refund the orders." },
        suspendForApproval: true,
        maxIterations: 4,
        ...(approvalScope !== undefined ? { approvalScope } : {})
      })
      .compile();

  it("asks for each call, and a signature unlocks that call only", async () => {
    const calls: unknown[] = [];
    const app = appWith(calls, "call");

    const first = await app.run({ task: "refund" }, { runId: "run_per_call" as never });
    expect(first.status).toBe("suspended");
    const filedA = filedOf(first.channels);
    expect(filedA?.input).toEqual(REFUNDS[0]);
    expect(filedA?.approvalKey).toMatch(/^refund#[0-9a-f]{64}$/);
    expect(filedA?.callKey).toBe(filedA?.approvalKey);

    // A grant by name unlocks nothing: the same call waits again.
    const byName = await app.approveAndResume(first.runId, {
      approvedTools: ["refund"],
      resolvedBy: "alice"
    });
    expect(byName.status).toBe("suspended");
    expect(calls).toEqual([]);

    // A's key: A is refunded; B — other arguments — waits for its own signature.
    const second = await app.approveAndResume(first.runId, {
      approvedTools: [{ name: "refund", key: filedA!.approvalKey! }],
      resolvedBy: "alice"
    });
    expect(second.status).toBe("suspended");
    expect(calls).toEqual([REFUNDS[0]]);
    const filedB = filedOf(second.channels);
    expect(filedB?.input).toEqual(REFUNDS[1]);
    expect(filedB?.approvalKey).toMatch(/^refund#[0-9a-f]{64}$/);
    expect(filedB?.approvalKey).not.toBe(filedA?.approvalKey);

    // R9 — documents a known limit, closed by ADR 0053 (the agent's continuation): a resumed
    // agent runs again from its first LLM call, so the grants of the current wait are not enough.
    // B's key alone: the agent asks for A again — A's signature is not given back.
    const onlyB = await app.approveAndResume(first.runId, {
      approvedTools: [{ name: "refund", key: filedB!.approvalKey! }],
      resolvedBy: "alice"
    });
    expect(onlyB.status).toBe("suspended");
    expect(filedOf(onlyB.channels)?.approvalKey).toBe(filedA?.approvalKey);
    expect(calls).toEqual([REFUNDS[0]]);
    // Both keys: the run completes, and A is refunded a second time.
    const both = await app.approveAndResume(first.runId, {
      approvedTools: [
        { name: "refund", key: filedA!.approvalKey! },
        { name: "refund", key: filedB!.approvalKey! }
      ],
      resolvedBy: "alice"
    });
    expect(both.status).toBe("completed");
    expect(calls).toEqual([REFUNDS[0], REFUNDS[0], REFUNDS[1]]);
  });

  it("refuses approvalScope \"call\" on a mapAgents sub-agent until ADR 0053 D6 (R10)", () => {
    const build = (approvalScope: "tool" | "call") => () =>
      createGraph({ name: "fan-out" })
        .channel("items", { type: "json", default: [] })
        .mapAgents("refunds", {
          overChannel: "items",
          joinAt: "results",
          suspendForApproval: true,
          subAgent: {
            model: model.openaiCompatible({ baseURL, model: "llama-3" }),
            tools: refundTools([]),
            prompt: { system: "Refund the order." },
            approvalScope
          }
        });
    expect(build("call")).toThrow(ApprovalScopeUnsupportedError);
    expect(build("tool")).not.toThrow();
  });

  it("keeps one name grant for every call under the default scope", async () => {
    const calls: unknown[] = [];
    const app = appWith(calls);

    const first = await app.run({ task: "refund" }, { runId: "run_per_tool" as never });
    expect(first.status).toBe("suspended");
    expect(filedOf(first.channels)?.approvalKey).toBeUndefined();

    const done = await app.approveAndResume(first.runId, {
      approvedTools: ["refund"],
      resolvedBy: "alice"
    });
    expect(done.status).toBe("completed");
    expect(calls).toEqual(REFUNDS);
  });
});
