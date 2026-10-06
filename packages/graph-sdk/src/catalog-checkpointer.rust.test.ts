import { describe, expect, it } from "vitest";

import {
  resumeCatalogGraph,
  runCatalogGraph,
  rustEngineAvailable,
  type Checkpoint,
  type GraphDefinition,
  type HostNodeInput
} from "./index.js";

/**
 * ADR 0049 D1 — the host keeps each checkpoint. `RunCatalogGraphOptions.checkpointer` receives
 * every checkpoint the run writes, awaited before the run goes on; a `save` that rejects stops
 * the run.
 */
const channels = {
  proposal: { type: "string", reducer: "replace" },
  receipt: { type: "string", reducer: "replace" }
};

/** One plain step, `send`, bound to the caller's code. */
const sendGraph = {
  id: "checkpointer-send",
  version: "1",
  name: "checkpointer-send",
  channels,
  nodes: [{ id: "send", type: "action", label: "send" }],
  edges: [],
  entryNodeId: "send"
} as unknown as GraphDefinition;

/** A human gate, then the step that acts. */
const gatedSendGraph = {
  id: "checkpointer-gated-send",
  version: "1",
  name: "checkpointer-gated-send",
  channels,
  nodes: [
    { id: "review", type: "human-gate", label: "review" },
    { id: "send", type: "action", label: "send" }
  ],
  edges: [{ id: "e1", from: "review", to: "send", type: "default" }],
  entryNodeId: "review"
} as unknown as GraphDefinition;

const recordingSend = () => {
  const calls: HostNodeInput[] = [];
  return {
    calls,
    binding: {
      id: "send",
      execute: async (input: HostNodeInput) => {
        calls.push(input);
        return { receipt: "r-1" };
      }
    }
  };
};

/** A store that keeps every checkpoint it is given. */
const keepingStore = () => {
  const kept: Checkpoint[] = [];
  return {
    kept,
    store: {
      save: async (checkpoint: Checkpoint) => {
        kept.push(checkpoint);
      }
    }
  };
};

const native = rustEngineAvailable() ? it : it.skip;

describe("@ailu-ai/graph-sdk — the host keeps each checkpoint (ADR 0049 D1)", () => {
  native("hands every checkpoint to the store, the last one being the outcome's", async () => {
    const send = recordingSend();
    const { kept, store } = keepingStore();

    const outcome = await runCatalogGraph(sendGraph, {
      runId: "run-kept" as never,
      initialData: { proposal: "p-1" },
      nodes: [send.binding],
      checkpointer: store
    });

    expect(outcome.status).toBe("completed");
    expect(kept.map((checkpoint) => checkpoint.graphState.status)).toEqual([
      "running",
      "completed"
    ]);
    expect(kept.every((checkpoint) => checkpoint.runId === "run-kept")).toBe(true);
    expect(kept.at(-1)?.id).toBe(outcome.state.checkpointId);
    expect(kept.at(-1)?.graphState.channels.receipt).toBe("r-1");
  });

  native("a store that cannot keep a checkpoint stops the run before its next node", async () => {
    const send = recordingSend();

    await expect(
      runCatalogGraph(sendGraph, {
        runId: "run-store-down" as never,
        initialData: { proposal: "p-1" },
        nodes: [send.binding],
        checkpointer: {
          save: async () => {
            throw new Error("store down");
          }
        }
      })
    ).rejects.toThrow(/could not keep checkpoint/);
    expect(send.calls).toHaveLength(0);
  });

  native("a run without a store is never asked to keep one", async () => {
    const send = recordingSend();

    const outcome = await runCatalogGraph(sendGraph, {
      initialData: { proposal: "p-1" },
      nodes: [send.binding]
    });

    expect(outcome.status).toBe("completed");
  });

  native("a suspension is kept, and a resume hands its checkpoints over too", async () => {
    const { kept, store } = keepingStore();

    const suspended = await runCatalogGraph(gatedSendGraph, {
      runId: "run-gated-kept" as never,
      initialData: { proposal: "p-1" },
      nodes: [recordingSend().binding],
      checkpointer: store
    });
    expect(suspended.status).toBe("suspended");
    expect(kept.at(-1)?.graphState.status).toBe("suspended");

    const before = kept.length;
    const finished = await resumeCatalogGraph(gatedSendGraph, suspended.state, {
      nodes: [recordingSend().binding],
      checkpointer: store
    });
    expect(finished.status).toBe("completed");
    expect(kept.length).toBeGreaterThan(before);
    expect(kept.at(-1)?.graphState.status).toBe("completed");
  });
});
