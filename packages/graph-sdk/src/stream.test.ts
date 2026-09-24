import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { Message, MessageId } from "@ailu-ai/graph-core";

import {
  createGraph,
  DefaultLLMGateway,
  rustEngineAvailable,
  type RunId,
  type StreamEvent
} from "./index.js";

/**
 * Incremental streaming over the Rust engine (ADR 0015): `values` accumulates a full
 * snapshot per node via the channel reducers; `messages` emits a `message_delta` per new
 * `messages` entry. `updates`/`debug` covered in subgraph.test.ts.
 */

const withRustEngine = () => {
  let saved: string | undefined;
  beforeEach(() => {
    saved = process.env.AILU_SDK_ENGINE;
    process.env.AILU_SDK_ENGINE = "rust";
  });
  afterEach(() => {
    if (saved === undefined) {
      delete process.env.AILU_SDK_ENGINE;
    } else {
      process.env.AILU_SDK_ENGINE = saved;
    }
  });
};

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("@ailu-ai/graph-sdk — incremental streaming (Rust engine)", () => {
  withRustEngine();

  it("`values` accumulates a full snapshot per node step", async () => {
    const app = createGraph({ name: "stream-values" })
      .channel("a", { type: "number", default: 0 })
      .channel("b", { type: "number", default: 0 })
      .node("n1", async () => ({ a: 1 }))
      .node("n2", async () => ({ b: 2 }))
      .edge("n1", "n2")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const events: StreamEvent[] = [];
    for await (const event of app.stream({}, "values", { runId: "run_values" as RunId })) {
      events.push(event);
    }
    const values = events.filter((e) => e.type === "state_value");
    expect(values.length).toBeGreaterThanOrEqual(2);
    // An intermediate snapshot after n1: a=1, b still at its default 0.
    expect(
      values.some(
        (e) =>
          e.type === "state_value" && e.state.channels.a === 1 && e.state.channels.b === 0
      )
    ).toBe(true);
    // The final snapshot has both.
    const last = values[values.length - 1];
    expect(last?.type).toBe("state_value");
    if (last?.type === "state_value") {
      expect(last.state.channels.a).toBe(1);
      expect(last.state.channels.b).toBe(2);
    }
  });

  it("`messages` emits a message_delta per new messages-channel entry", async () => {
    const message: Message = {
      id: "m1" as MessageId,
      createdAt: new Date(),
      role: "ai",
      content: "hello world"
    };
    const app = createGraph({ name: "stream-messages" })
      .messagesChannel()
      .node("say", async () => ({ messages: [message] }))
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const events: StreamEvent[] = [];
    for await (const event of app.stream({}, "messages", { runId: "run_messages" as RunId })) {
      events.push(event);
    }
    const deltas = events.filter((e) => e.type === "message_delta");
    expect(deltas.length).toBe(1);
    if (deltas[0]?.type === "message_delta") {
      expect(deltas[0].delta).toBe("hello world");
      expect(deltas[0].nodeId).toBe("say");
    }
  });

  it("`messages` mode streams per-token deltas from an agent node (ADR 0033 phase 13)", async () => {
    // Force the deterministic mock gateway (no provider keys), so the agent's generation
    // is reproducible offline. The mock streams chunk-once (the whole content as one
    // delta), which the engine surfaces as a `token_delta` → projected to `message_delta`.
    const savedKeys: Record<string, string | undefined> = {};
    for (const key of ["ANTHROPIC_API_KEY", "MISTRAL_API_KEY", "AILU_USE_OLLAMA"]) {
      savedKeys[key] = process.env[key];
      delete process.env[key];
    }
    try {
      const app = createGraph({ name: "stream-tokens" })
        .agentNode("assistant", {
          llm: new DefaultLLMGateway(),
          prompt: { system: "You are a brief assistant." },
          maxIterations: 1
        })
        .compile();

      expect(app.usesRustEngine).toBe(true);
      const events: StreamEvent[] = [];
      for await (const event of app.stream({ question: "hi" }, "messages", {
        runId: "run_tokens" as RunId
      })) {
        events.push(event);
      }

      // The agent writes no `messages` channel; every message_delta here therefore comes
      // from a token_delta projection — proving the per-token path is wired end to end.
      const deltas = events.filter((e) => e.type === "message_delta");
      expect(deltas.length).toBeGreaterThanOrEqual(1);
      const first = deltas[0];
      expect(first?.type).toBe("message_delta");
      if (first?.type === "message_delta") {
        expect(first.delta.length).toBeGreaterThan(0);
        expect(first.nodeId).toBe("assistant");
        // The messageId groups a turn's tokens (namespaced with the run id by the sink).
        expect(first.messageId.length).toBeGreaterThan(0);
      }
    } finally {
      for (const [key, value] of Object.entries(savedKeys)) {
        if (value === undefined) {
          delete process.env[key];
        } else {
          process.env[key] = value;
        }
      }
    }
  });

  it("concurrent streams of one compiled graph only see their own run's events", async () => {
    const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const app = createGraph({ name: "stream-concurrent" })
      .channel("who", { type: "string", default: "" })
      .channel("seen", { type: "string", default: "" })
      .node("first", async (_input, state) => {
        await sleep(20);
        return { seen: state.channels.who };
      })
      .node("second", async (_input, state) => {
        await sleep(20);
        return { seen: `${state.channels.seen}!` };
      })
      .edge("first", "second")
      .compile();

    expect(app.usesRustEngine).toBe(true);
    const collect = async (who: string): Promise<StreamEvent[]> => {
      const events: StreamEvent[] = [];
      for await (const event of app.stream({ who }, "updates", { runId: `run_${who}` as RunId })) {
        events.push(event);
      }
      return events;
    };
    const [alice, bob] = await Promise.all([collect("alice"), collect("bob")]);

    const seenValues = (events: StreamEvent[]): unknown[] =>
      events.flatMap((e) => (e.type === "state_update" ? [e.delta.seen] : []));
    expect(seenValues(alice)).toEqual(["alice", "alice!"]);
    expect(seenValues(bob)).toEqual(["bob", "bob!"]);
  });

  it("a consumer that stops iterating early does not leave an unhandled rejection", async () => {
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => unhandled.push(reason);
    process.on("unhandledRejection", onUnhandled);
    try {
      // A self-loop past its recursion limit makes the engine call itself reject (a runtime
      // error, not a failed-run state) after the consumer has already stopped iterating.
      const app = createGraph({ name: "stream-early-exit", recursionLimit: 3 })
        .channel("a", { type: "number", default: 0 })
        .node("tick", async (_input, state) => {
          await new Promise((resolve) => setTimeout(resolve, 5));
          return { a: Number(state.channels.a) + 1 };
        })
        .edge("tick", "tick")
        .compile();

      for await (const event of app.stream({}, "updates", { runId: "run_early_exit" as RunId })) {
        expect(event.type).toBe("state_update");
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
      expect(unhandled).toEqual([]);
    } finally {
      process.off("unhandledRejection", onUnhandled);
    }
  });
});
