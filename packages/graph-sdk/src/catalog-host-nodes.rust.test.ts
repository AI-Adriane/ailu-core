import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  HostNodeBindingError,
  replayCatalogGraph,
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type GraphDefinition,
  type HostNodeInput,
  type RunId
} from "./index.js";

/**
 * ADR 0045 D2.3 — host nodes on the CATALOG run path. `RunCatalogGraphOptions.nodes` binds the
 * caller's code to a plain action/tool node by id: it receives the node's channels and the
 * execution's effect key, and returns the channel update. The engine journals each execution in
 * record mode and a replay serves the journal — it never calls the node again (ADR 0045 D1).
 */
const channels = {
  proposal: { type: "string", reducer: "replace" },
  receipt: { type: "string", reducer: "replace" }
};

/** One plain step, `send`: the shape of a step that writes to the outside world. */
const sendGraph = {
  id: "host-node-send",
  version: "1",
  name: "host-node-send",
  channels,
  nodes: [{ id: "send", type: "action", label: "send" }],
  edges: [],
  entryNodeId: "send"
} as unknown as GraphDefinition;

/** A human gate, then the step that acts: ADR 0096's "the agent proposes, a human signs". */
const gatedSendGraph = {
  id: "host-node-gated-send",
  version: "1",
  name: "host-node-gated-send",
  channels,
  nodes: [
    { id: "review", type: "human-gate", label: "review" },
    { id: "send", type: "action", label: "send" }
  ],
  edges: [{ id: "e1", from: "review", to: "send", type: "default" }],
  entryNodeId: "review"
} as unknown as GraphDefinition;

/** A `send` binding that records each call and answers with a receipt. */
const recordingSend = (receipt = "r-1") => {
  const calls: HostNodeInput[] = [];
  return {
    calls,
    binding: {
      id: "send",
      execute: async (input: HostNodeInput) => {
        calls.push(input);
        return { receipt };
      }
    }
  };
};

const native = rustEngineAvailable() ? it : it.skip;

describe("@ailu-ai/graph-sdk — catalog host nodes (ADR 0045 D2.3)", () => {
  let savedRecord: string | undefined;
  beforeEach(() => {
    savedRecord = process.env.AILU_LLM_RECORD;
    delete process.env.AILU_LLM_RECORD;
  });
  afterEach(() => {
    if (savedRecord === undefined) delete process.env.AILU_LLM_RECORD;
    else process.env.AILU_LLM_RECORD = savedRecord;
  });

  native(
    "runs a bound plain node with its channels and effect key, and applies its update",
    async () => {
      const send = recordingSend();
      const outcome = await runCatalogGraph(sendGraph, {
        runId: "run_host_node_bound" as RunId,
        initialData: { proposal: "p-1" },
        nodes: [send.binding]
      });
      expect(outcome.status).toBe("completed");
      expect(outcome.state.channels.receipt).toBe("r-1");
      expect(send.calls).toHaveLength(1);
      expect(send.calls[0]?.nodeId).toBe("send");
      expect(send.calls[0]?.channels.proposal).toBe("p-1");
      expect(send.calls[0]?.effectKey).toMatch(/^[0-9a-f]{64}$/);
    }
  );

  native(
    "gives a retry of the same run and step the same effect key, another run another",
    async () => {
      const send = recordingSend();
      for (const runId of ["run_host_node_key", "run_host_node_key", "run_host_node_other"]) {
        await runCatalogGraph(sendGraph, {
          runId: runId as RunId,
          initialData: { proposal: "p-1" },
          nodes: [send.binding]
        });
      }
      const [first, retry, other] = send.calls.map((call) => call.effectKey);
      expect(retry).toBe(first);
      expect(other).not.toBe(first);
    }
  );

  native("keeps an unbound plain node an empty step", async () => {
    const outcome = await runCatalogGraph(sendGraph, {
      runId: "run_host_node_unbound" as RunId,
      initialData: { proposal: "p-1" }
    });
    expect(outcome.status).toBe("completed");
    expect(outcome.state.channels.receipt).toBeNull();
  });

  native("acts only after the gate: the binding runs on the resume, once", async () => {
    const send = recordingSend();
    const suspended = await runCatalogGraph(gatedSendGraph, {
      runId: "run_host_node_gated" as RunId,
      initialData: { proposal: "p-1" },
      nodes: [send.binding]
    });
    expect(suspended.status).toBe("suspended");
    expect(send.calls).toHaveLength(0);

    const resumed = await resumeCatalogGraph(gatedSendGraph, suspended.state, {
      nodes: [send.binding]
    });
    expect(resumed.status).toBe("completed");
    expect(resumed.state.channels.receipt).toBe("r-1");
    expect(send.calls).toHaveLength(1);
  });

  native("fails the run when the binding throws", async () => {
    const outcome = await runCatalogGraph(sendGraph, {
      runId: "run_host_node_throws" as RunId,
      initialData: { proposal: "p-1" },
      nodes: [
        {
          id: "send",
          execute: async () => {
            throw new Error("slack 503");
          }
        }
      ]
    });
    expect(outcome.status).toBe("failed");
  });

  native(
    "journals the step in record mode; its replay serves the receipt without calling it",
    async () => {
      const send = recordingSend();
      process.env.AILU_LLM_RECORD = "1";
      const recorded = await runCatalogGraph(sendGraph, {
        runId: "run_host_node_replay" as RunId,
        initialData: { proposal: "p-1" },
        nodes: [send.binding]
      });
      delete process.env.AILU_LLM_RECORD;
      expect(recorded.status).toBe("completed");
      expect(send.calls).toHaveLength(1);
      const journal = JSON.parse(recorded.replayJournal ?? "{}") as {
        nodeResults?: { nodeId: string; update?: unknown }[];
      };
      expect(journal.nodeResults).toEqual([
        expect.objectContaining({ nodeId: "send", update: { receipt: "r-1" } })
      ]);

      // The replay is handed no binding: were the step called, it would be an empty step and
      // `receipt` would stay empty. The receipt is there — served from the journal.
      const replayed = await replayCatalogGraph(
        sendGraph,
        recorded.entryState ?? recorded.state,
        "cp_host_node_replay",
        recorded.replayJournal ?? "{}"
      );
      expect(replayed.status).toBe("completed");
      expect(replayed.state.channels.receipt).toBe("r-1");
      expect(send.calls).toHaveLength(1);
    }
  );

  native("refuses a binding that names no plain node, or names one twice", async () => {
    const run = (
      nodes: { id: string; execute: () => Record<string, unknown> }[],
      definition = sendGraph
    ) => runCatalogGraph(definition, { runId: "run_host_node_refused" as RunId, nodes });
    const execute = () => ({});
    await expect(run([{ id: "missing", execute }])).rejects.toBeInstanceOf(HostNodeBindingError);
    await expect(run([{ id: "review", execute }], gatedSendGraph)).rejects.toThrow(
      /not a plain step/
    );
    await expect(
      run([
        { id: "send", execute },
        { id: "send", execute }
      ])
    ).rejects.toThrow(/bound twice/);
  });
});
