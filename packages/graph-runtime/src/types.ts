import type { ChannelsSchema, EdgeId, FailureCategory, GraphState, NodeId, RunId } from "@ailu-ai/graph-core";

export type CheckpointId = string & { readonly __brand: "CheckpointId" };

export type Checkpoint<TChannels extends ChannelsSchema = ChannelsSchema> = {
  id: CheckpointId;
  runId: RunId;
  graphState: GraphState<TChannels>;
  createdAt: string;
};

export type RunEvent =
  | { type: "node_started"; runId: RunId; nodeId: NodeId; timestamp: string }
  | {
      type: "node_completed";
      runId: RunId;
      nodeId: NodeId;
      output: unknown;
      timestamp: string;
    }
  | {
      type: "node_failed";
      runId: RunId;
      nodeId: NodeId;
      error: string;
      attempt: number;
      category: FailureCategory;
      timestamp: string;
    }
  // ADR 0076 (product repo) — emitted instead of `run_failed` when `retryPolicy` is exhausted AND
  // the failed node has an outgoing `"error"` edge: the run is rerouted to `toNodeId` rather than
  // terminated. This is the auditable "governed error branch" signal — a retry alone (already
  // covered by `node_failed`'s repeated emissions per attempt) stays silent-ish; an actual reroute
  // of run control flow gets its own event so it's unambiguous in the journal which happened.
  | {
      type: "node_error_routed";
      runId: RunId;
      nodeId: NodeId;
      errorEdgeId: EdgeId;
      toNodeId: NodeId;
      category: FailureCategory;
      error: string;
      timestamp: string;
    }
  | {
      type: "run_suspended";
      runId: RunId;
      nodeId: NodeId;
      reason: string;
      timestamp: string;
    }
  | { type: "run_resumed"; runId: RunId; nodeId: NodeId; timestamp: string }
  | {
      type: "run_completed";
      runId: RunId;
      finalState: GraphState<ChannelsSchema>;
      timestamp: string;
    }
  | { type: "run_failed"; runId: RunId; error: string; timestamp: string }
  // ADR 0044: terminal cooperative cancellation at a node BOUNDARY. `nodeId` is the node the
  // run was ABOUT to execute — it never ran. Emitted exactly once, after the final checkpoint
  // is durable, and never alongside `run_completed` / `run_failed`.
  | { type: "run_cancelled"; runId: RunId; nodeId: NodeId; timestamp: string }
  // ADR 0033 phase 13: one observational per-token delta during agent generation.
  // Observational-only — never persisted (it bypasses the EventBus on the Rust path),
  // so it is absent from checkpoints and the journal. `messageId` groups all deltas of
  // one agent turn; `spawnId`/`parentRunId` tag a `mapAgents` sub-agent's stream so a
  // consumer can demultiplex concurrent spawns (both absent for a top-level agent node).
  | {
      type: "token_delta";
      runId: RunId;
      nodeId: NodeId;
      messageId: string;
      delta: string;
      parentRunId?: RunId;
      spawnId?: number;
      timestamp: string;
    };
