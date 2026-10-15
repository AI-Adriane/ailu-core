import { createHash } from "node:crypto";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

// Import the in-memory engine directly (not the package index) so the test never pulls the Pg
// engine and its `db`/`pg` dependency chain — same discipline as child-workflow-approval.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

import {
  callInputOf,
  callKeyOf,
  createGraph,
  InMemoryToolRegistry,
  model,
  runCatalogGraph,
  rustEngineAvailable,
  type AgentResult,
  type GraphDefinition
} from "./index.js";

/**
 * ADR 0051 D1 on the Rust engine, end to end: a tool gated by its NAME files the call the signer
 * approves — its arguments, the canonical text that is hashed (`callInput`) and its call key
 * `<name>#<sha256(callInput)>` — on the agent's result, on `pendingApprovals` and on the request a
 * catalog run files in its approval engine, while the grant stays the name (no `approvalKey`; a
 * name grant still unlocks the tool). A local OpenAI-compatible server plays the model: it asks
 * for `refund`, then answers once it has seen the tool's result.
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

/** The model writes the arguments in this order; the key does not depend on it. */
const REFUND_ARGUMENTS = '{"order":"A-7","amount":40}';

const refundCall = {
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
            function: { name: "refund", arguments: REFUND_ARGUMENTS }
          }
        ]
      },
      finish_reason: "tool_calls"
    }
  ],
  usage
};

const answer = {
  model: "llama-3",
  choices: [{ message: { role: "assistant", content: "FINAL: refunded" }, finish_reason: "stop" }],
  usage
};

/** The canonical text the engine hashes for these arguments: keys sorted, compact. */
const CALL_INPUT = '{"amount":40,"order":"A-7"}';

/** What a host checks on its side, without reimplementing the form: sha256 over `callInput`. */
const keyOfCallInput = (name: string, callInput: string): string =>
  `${name}#${createHash("sha256").update(callInput, "utf8").digest("hex")}`;

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

type FiledCall = {
  subject: unknown;
  approvalKey?: string;
  input?: unknown;
  callInput?: string;
  callKey?: string;
};

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("a gate by name files the call it stops (ADR 0051 D1) on the Rust engine", () => {
  let server: Server;
  let baseURL = "";

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((raw) => {
        const body = JSON.parse(raw) as { messages?: Array<{ role?: string }> };
        const sawResult = (body.messages ?? []).some((message) => message.role === "tool");
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(sawResult ? answer : refundCall));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    baseURL = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  it("files the arguments and the call key, and still unlocks the tool by its name", async () => {
    const calls: unknown[] = [];
    const app = createGraph({ name: "signed-tool-input" })
      .agentNode("assistant", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        tools: refundTools(calls),
        prompt: { system: "Refund the order." },
        suspendForApproval: true,
        maxIterations: 3
      })
      .compile();
    expect(app.usesRustEngine).toBe(true);

    const suspended = await app.run({ task: "refund" }, { runId: "run_signed_input" as never });

    expect(suspended.status).toBe("suspended");
    const agentResult = (suspended.channels as Record<string, AgentResult | undefined>).agentResult;
    const filed = agentResult?.approvalRequests[0] as FiledCall | undefined;
    expect(filed?.subject).toEqual("tool:refund");
    expect(filed?.input).toEqual({ order: "A-7", amount: 40 });
    expect(filed?.callInput).toBe(CALL_INPUT);
    expect(callInputOf(REFUND_ARGUMENTS)).toBe(CALL_INPUT);
    expect(filed?.callKey).toBe(keyOfCallInput("refund", CALL_INPUT));
    expect(filed?.callKey).toBe(callKeyOf("refund", REFUND_ARGUMENTS));
    // The grant is still the name: no key to give back.
    expect(filed?.approvalKey).toBeUndefined();
    expect(calls).toEqual([]);

    const done = await app.approveAndResume(suspended.runId, {
      approvedTools: ["refund"],
      resolvedBy: "alice"
    });
    expect(done.status).toBe("completed");
    expect(calls).toEqual([{ order: "A-7", amount: 40 }]);
  });

  it("files the call in the approval engine of a catalog run", async () => {
    const graph = {
      id: "signed-tool-input-catalog",
      version: "1",
      name: "signed-tool-input-catalog",
      channels: {
        agentResult: { type: "agentResult", reducer: "replace" },
        __approvedTools: { type: "string[]", reducer: "replace", default: [] },
        __approvalIds: { type: "string[]", reducer: "replace", default: [] }
      },
      nodes: [
        {
          id: "assistant",
          type: "agent",
          label: "assistant",
          metadata: {
            agent: {
              provider: "openai",
              model: "llama-3",
              baseURL,
              toolNames: ["refund"],
              suspendForApproval: true,
              approvalToolNames: ["refund"],
              outputChannel: "agentResult",
              maxIterations: 3
            }
          }
        }
      ],
      edges: [],
      entryNodeId: "assistant"
    } as unknown as GraphDefinition;
    const engine = new InMemoryApprovalEngine();

    const outcome = await runCatalogGraph(graph, {
      runId: "run_signed_input_catalog" as never,
      approvalEngine: engine
    });

    expect(outcome.status).toBe("suspended");
    const callKey = keyOfCallInput("refund", CALL_INPUT);
    expect(outcome.pendingApprovals?.[0]?.callKey).toBe(callKey);
    expect(outcome.pendingApprovals?.[0]?.callInput).toBe(CALL_INPUT);
    const pending = await engine.getPending("run_signed_input_catalog" as never);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.subject).toEqual({
      description: "tool:refund",
      input: { order: "A-7", amount: 40 },
      callInput: CALL_INPUT,
      callKey
    });
  });
});
