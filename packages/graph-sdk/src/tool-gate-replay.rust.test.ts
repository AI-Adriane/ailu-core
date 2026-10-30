import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";

import {
  InMemoryApprovalEngine,
  InMemoryToolRegistry,
  attestedDecisions,
  createGraph,
  model,
  replayCatalogGraph,
  replayedDecisions,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  verifyReplayDecisions,
  type AgentResult,
  type GraphDefinition,
  type RunId
} from "./index.js";

/**
 * Replay-as-evidence of a run whose agent calls a tool gated by `approvalWhen`, on a host tool,
 * with `suspendForApproval`: the run suspends at the tool's gate, a member approves that call, the
 * run resumes and the tool runs. The replay (no host tool bound, ADR 0041) must stop at the same
 * gate, so that the request it re-derives is the one the chain attests.
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
const REMINDER = { invoiceRef: "INV-1", amountCents: 124000, customerRef: "C-1" };

const reminderCall = {
  model: "llama-3",
  choices: [
    {
      message: {
        role: "assistant",
        content: "All refs present. Calling now.",
        tool_calls: [
          {
            id: "call_1",
            type: "function",
            function: { name: "send_reminder", arguments: JSON.stringify(REMINDER) }
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
  choices: [{ message: { role: "assistant", content: "FINAL: sent" }, finish_reason: "stop" }],
  usage
};

const native = rustEngineAvailable() ? describe : describe.skip;

it.runIf(process.env.AILU_REQUIRE_NATIVE === "1")(
  "runs on the native engine (AILU_REQUIRE_NATIVE=1)",
  () => {
    expect(rustEngineAvailable()).toBe(true);
  }
);

native("replay of a run gated on a conditioned host tool", () => {
  let server: Server;
  let baseURL = "";
  const executed: unknown[] = [];

  beforeAll(async () => {
    server = createServer((request, response) => {
      void readBody(request).then((raw) => {
        const body = JSON.parse(raw) as { messages?: Array<{ role?: string }> };
        const sawResult = (body.messages ?? []).some((message) => message.role === "tool");
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify(sawResult ? answer : reminderCall));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    baseURL = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  afterEach(() => {
    delete process.env.AILU_LLM_RECORD;
    executed.length = 0;
  });

  const build = (extraTools = 0) => {
    const tools = new InMemoryToolRegistry();
    for (let index = 0; index < extraTools; index += 1) {
      tools.register(
        {
          id: `lookup_${index}` as never,
          name: `lookup_${index}`,
          description: `Look something up (${index}).`,
          inputSchema: { parse: (value: unknown) => value as Record<string, unknown> },
          outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
          permissions: [],
          jsonSchema: { type: "object" }
        },
        async () => ({ ok: true })
      );
    }
    tools.register(
      {
        id: "send_reminder" as never,
        name: "send_reminder",
        description: "Send a payment reminder.",
        inputSchema: { parse: (value: unknown) => value as Record<string, unknown> },
        outputSchema: { parse: (value: unknown) => value as { ok: boolean } },
        permissions: [],
        requiresApproval: true,
        approvalWhen: [{ argument: "amountCents", above: 0 }],
        jsonSchema: {
          type: "object",
          properties: { invoiceRef: { type: "string" }, amountCents: { type: "number" } },
          required: ["invoiceRef", "amountCents"]
        }
      },
      async (input) => {
        executed.push(input);
        return { ok: true };
      }
    );
    const definition: GraphDefinition = createGraph({ name: "tool-gate-replay" })
      .agentNode("carrier", {
        model: model.openaiCompatible({ baseURL, model: "llama-3" }),
        tools,
        prompt: { system: "Send the reminder." },
        suspendForApproval: true,
        maxIterations: 3
      })
      .compile().definition;
    return {
      definition,
      bindings: [
        ...Array.from({ length: extraTools }, (_, index) => ({
          name: `lookup_${index}`,
          execute: async () => ({ ok: true })
        })),
        {
          name: "send_reminder",
          execute: async (input: unknown) => {
            executed.push(input);
            return { ok: true };
          }
        }
      ]
    };
  };

  {
    it("replays a single-tool agent to the gate the chain attests (one signature)", async () => {
      process.env.AILU_LLM_RECORD = "1";
      const { definition, bindings } = build();
      const approvals = new InMemoryApprovalEngine();
      const runId = "tool-gate-replay-run" as RunId;
      const started = await runCatalogGraph(definition, {
        runId,
        initialData: { input: "Remind." },
        tools: bindings,
        approvalEngine: approvals
      });
      expect(started.status).toBe("suspended");
      expect(executed).toEqual([]);
      const [filed] = await approvals.getPending(runId);
      if (filed === undefined) throw new Error("the tool gate filed no request");
      const approved = await approvals.approve(filed.id, "member");
      const key = (approved.subject as { callKey?: string }).callKey;
      const resumed = await resumeCatalogGraph(definition, started.state, {
        tools: bindings,
        approvalEngine: approvals,
        approvedTools: [
          { name: "send_reminder", requestedBy: "carrier", resolvedBy: "member", key }
        ]
      });
      expect(resumed.status).toBe("completed");
      expect(executed).toHaveLength(1);

      const attested = attestedDecisions([
        {
          runId,
          approvalId: approved.id,
          subject: (approved.subject as { description: string }).description,
          callKey: key,
          grant: undefined,
          resolvedBy: "member",
          status: "approved"
        } as never
      ]);
      const replay = await replayCatalogGraph(
        definition,
        started.entryState ?? started.state,
        `${runId}:entry`,
        started.replayJournal ?? "{}"
      );
      const agentResult = (replay.state.channels as Record<string, AgentResult | undefined>)
        .agentResult;
      expect(agentResult?.approvalRequests).toHaveLength(1);
      expect(replay.status).toBe("suspended");
      const replayed = replayedDecisions(replay, definition);
      expect(verifyReplayDecisions(attested, replayed).ok).toBe(true);
      // The replay ran no tool.
      expect(executed).toHaveLength(1);
    });
  }

  /** A run with several tools, recorded and approved on its tool gate; its replay inputs. */
  const recordWithTools = async (extraTools: number, name: string) => {
    process.env.AILU_LLM_RECORD = "1";
    const { definition, bindings } = build(extraTools);
    const runId = name as RunId;
    const started = await runCatalogGraph(definition, {
      runId,
      initialData: { input: "Remind." },
      tools: bindings,
      approvalEngine: new InMemoryApprovalEngine()
    });
    expect(started.status).toBe("suspended");
    return { definition, runId, started };
  };

  it("replays an agent with several tools to its gate, whatever order the tools are listed in", async () => {
    const { definition, runId, started } = await recordWithTools(4, "tool-gate-many-run");
    for (let attempt = 0; attempt < 12; attempt += 1) {
      const replay = await replayCatalogGraph(
        definition,
        started.entryState ?? started.state,
        `${runId}:entry`,
        started.replayJournal ?? "{}"
      );
      const agentResult = (replay.state.channels as Record<string, AgentResult | undefined>)
        .agentResult;
      expect(agentResult).not.toHaveProperty("error");
      expect(replay.status).toBe("suspended");
      expect(replayedDecisions(replay, definition)).toEqual([
        {
          status: "",
          subject: "tool:send_reminder",
          callKey: expect.stringMatching(/^send_reminder#[0-9a-f]{64}$/)
        }
      ]);
    }
  });

  it("replays a journal recorded with the tools listed in another order", async () => {
    // A journal written by an engine that listed an agent's tools in no fixed order: the same
    // tools, advertised in the opposite order, are still the recorded request.
    const { definition, runId, started } = await recordWithTools(4, "tool-gate-reordered-run");
    const journal = JSON.parse(started.replayJournal ?? "{}") as {
      decisions: { calls: Array<{ request: { tools: unknown[] } }> };
    };
    for (const call of journal.decisions.calls) call.request.tools.reverse();
    const replay = await replayCatalogGraph(
      definition,
      started.entryState ?? started.state,
      `${runId}:entry`,
      JSON.stringify(journal)
    );
    expect(replay.status).toBe("suspended");
    expect(replayedDecisions(replay, definition)).toHaveLength(1);
  });
});
