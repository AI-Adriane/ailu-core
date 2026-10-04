import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  createGraph,
  InMemoryToolRegistry,
  model,
  rustEngineAvailable,
  type AgentResult
} from "./index.js";

/**
 * ADR 0048 on the Rust engine, end to end: a delegation to a NAMED agent is gated — filed with what
 * crossed and its key, and that call only is unlocked by the key — while a delegation to another
 * agent runs as an ungated call does. A local OpenAI-compatible server plays the model: it asks
 * for `a2a_delegate` to the agent the test names, then answers once it has seen the tool's result.
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

const delegationTo = (agentName: string) => ({
  model: "llama-3",
  choices: [
    {
      message: {
        role: "assistant",
        content: null,
        tool_calls: [
          {
            id: "call_1",
            type: "function",
            function: {
              name: "a2a_delegate",
              arguments: JSON.stringify({ agentName, message: "quote 40 pumps" })
            }
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
  choices: [{ message: { role: "assistant", content: "FINAL: sent" }, finish_reason: "stop" }],
  usage
};

/** The tool the product gates per agent (ADR 0111 D3): `a2a_delegate`, gated for Nordlys only. */
const delegationTools = (calls: unknown[]): InMemoryToolRegistry => {
  const tools = new InMemoryToolRegistry();
  tools.register(
    {
      id: "a2a_delegate" as never,
      name: "a2a_delegate",
      description: "Send a task to a registered remote agent.",
      inputSchema: { parse: (value: unknown) => value as Record<string, unknown> },
      outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
      permissions: [],
      requiresApproval: true,
      approvalWhen: [{ argument: "agentName", in: ["Nordlys"] }],
      jsonSchema: {
        type: "object",
        properties: { agentName: { type: "string" }, message: { type: "string" } },
        required: ["agentName", "message"]
      }
    },
    async (input) => {
      calls.push(input);
      return { ok: true };
    }
  );
  return tools;
};

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("approvalWhen with named values (ADR 0048) on the Rust engine", () => {
  let server: Server;
  let baseURL = "";
  let target = "Nordlys";

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((raw) => {
        const body = JSON.parse(raw) as { messages?: Array<{ role?: string }> };
        const sawResult = (body.messages ?? []).some((message) => message.role === "tool");
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(sawResult ? answer : delegationTo(target)));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    baseURL = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  const appWith = (calls: unknown[]) =>
    createGraph({ name: "named-values" })
      .agentNode("reply", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        tools: delegationTools(calls),
        prompt: { system: "Delegate the task." },
        suspendForApproval: true,
        maxIterations: 3
      })
      .compile();

  it("lets a delegation to another agent run, as an ungated call does", async () => {
    target = "Veritas";
    const calls: unknown[] = [];
    const app = appWith(calls);
    expect(app.usesRustEngine).toBe(true);

    const result = await app.run({ task: "quote" }, { runId: "run_named_other" as never });

    expect(result.status).toBe("completed");
    expect(calls).toEqual([{ agentName: "Veritas", message: "quote 40 pumps" }]);
  });

  it("gates a delegation to a named agent, files what crossed, and unlocks that call by its key", async () => {
    target = "Nordlys";
    const calls: unknown[] = [];
    const app = appWith(calls);

    const suspended = await app.run({ task: "quote" }, { runId: "run_named_gated" as never });

    expect(suspended.status).toBe("suspended");
    const agentResult = (suspended.channels as Record<string, AgentResult | undefined>).agentResult;
    const filed = agentResult?.approvalRequests[0] as {
      subject: unknown;
      approvalKey?: string;
      input?: unknown;
      condition?: string;
    };
    expect(filed.subject).toEqual("tool:a2a_delegate");
    expect(filed.condition).toBe('agentName = "Nordlys"');
    expect(filed.input).toEqual({ agentName: "Nordlys", message: "quote 40 pumps" });
    expect(filed.approvalKey).toMatch(/^a2a_delegate#[0-9a-f]{64}$/);
    expect(calls).toEqual([]);

    // A grant by name unlocks nothing: the engine gates the call again.
    const byName = await app.approveAndResume(suspended.runId, {
      approvedTools: ["a2a_delegate"],
      resolvedBy: "alice"
    });
    expect(byName.status).toBe("suspended");
    expect(calls).toEqual([]);

    // The call's key: that delegation is sent, once.
    const done = await app.approveAndResume(suspended.runId, {
      approvedTools: [{ name: "a2a_delegate", key: filed.approvalKey! }],
      resolvedBy: "alice"
    });
    expect(done.status).toBe("completed");
    expect(calls).toEqual([{ agentName: "Nordlys", message: "quote 40 pumps" }]);
  });
});
