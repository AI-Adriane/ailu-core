//! Runtime value types: checkpoints and the run-event vocabulary.

use adriane_graph_core::{EdgeId, FailureCategory, GraphState, NodeId, RunId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Identifier of a persisted checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(pub String);

/// A point-in-time snapshot of a run's state, persisted after every node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub run_id: RunId,
    pub graph_state: GraphState,
    pub created_at: String,
}

/// Lifecycle events emitted for every node transition. The `type` tag matches the
/// TS event vocabulary (`node_started`, `run_suspended`, …); the variant FIELDS
/// serialize camelCase (`runId`/`nodeId`) to match the TS `RunEvent` shape the SDK
/// parses across the napi boundary (`rename_all_fields`). Without this the fields
/// would emit snake_case and `event.nodeId` would be `undefined` on the JS side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RunEvent {
    NodeStarted {
        run_id: RunId,
        node_id: NodeId,
        timestamp: String,
    },
    NodeCompleted {
        run_id: RunId,
        node_id: NodeId,
        output: BTreeMap<String, Value>,
        timestamp: String,
    },
    NodeFailed {
        run_id: RunId,
        node_id: NodeId,
        error: String,
        /// 1-based attempt number — one `NodeFailed` is emitted per failed attempt.
        attempt: u32,
        /// ADR 0076. Defaults to `Unknown` when the handler didn't classify its failure.
        #[serde(default)]
        category: FailureCategory,
        timestamp: String,
    },
    /// ADR 0076 — emitted instead of `RunFailed` when `retryPolicy` is exhausted AND the
    /// failed node has an outgoing `"error"` edge: the run reroutes to `to_node_id` rather
    /// than terminating. The auditable "governed error branch" signal — unambiguous in the
    /// journal that a reroute happened, distinct from `NodeFailed`'s per-attempt emissions.
    NodeErrorRouted {
        run_id: RunId,
        node_id: NodeId,
        error_edge_id: EdgeId,
        to_node_id: NodeId,
        category: FailureCategory,
        error: String,
        timestamp: String,
    },
    RunSuspended {
        run_id: RunId,
        node_id: NodeId,
        reason: String,
        timestamp: String,
    },
    RunResumed {
        run_id: RunId,
        node_id: NodeId,
        timestamp: String,
    },
    RunCompleted {
        run_id: RunId,
        timestamp: String,
    },
    /// Terminal failure after retry attempts are exhausted — mirrors the TS
    /// `run_failed` event. A failed run never also emits `run_completed`.
    RunFailed {
        run_id: RunId,
        error: String,
        timestamp: String,
    },
    /// Terminal cancellation at a node boundary (ADR 0044). `node_id` is the node the run
    /// was ABOUT to execute when the cancellation seam was observed — it never ran. Emitted
    /// exactly once, after the final checkpoint is durable, and never alongside
    /// `RunCompleted`/`RunFailed`.
    RunCancelled {
        run_id: RunId,
        node_id: NodeId,
        timestamp: String,
    },
    /// One observational per-token delta during agent generation (ADR 0033, phase 13).
    ///
    /// **Observational only — never durable.** Unlike every other variant, a `TokenDelta`
    /// is NEVER put on the `EventBus` (so it never enters the in-memory events vector, a
    /// checkpoint, or the durable journal). It is built and serialized directly onto the
    /// napi `on_event` sink in `bindings` (the `agents-core` `EventSink` impl), bypassing
    /// the bus entirely — `durability ≠ observability`. The authoritative generation
    /// result is the assembled `LlmResponse`, which flows through the normal path and is
    /// what every checkpoint records; these deltas are a live read view of the same
    /// generation. Do **not** emit this via `EventBus::emit`.
    TokenDelta {
        run_id: RunId,
        node_id: NodeId,
        /// Groups all deltas of one agent turn so a consumer concatenates them.
        message_id: String,
        delta: String,
        /// The parent run for a `mapAgents` spawn (the run that fanned it out); `None`
        /// for a top-level agent node. Additive to the `<parentRunId>:<nodeId>` RunId
        /// convention, not a replacement.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<RunId>,
        /// Which `mapAgents` sub-agent produced this delta (input index = merge order);
        /// `None` for a top-level agent node.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_id: Option<u32>,
        timestamp: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_event_serializes_snake_case_tag_with_camel_case_fields() {
        let event = RunEvent::NodeCompleted {
            run_id: RunId::from("r"),
            node_id: NodeId::from("n"),
            output: BTreeMap::new(),
            timestamp: "0".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap();
        // Variant tag stays snake_case; fields are camelCase to match the TS `RunEvent`
        // the SDK parses over napi (so `event.nodeId` is defined on the JS side).
        assert!(json.contains("\"type\":\"node_completed\""));
        assert!(json.contains("\"runId\":\"r\""));
        assert!(json.contains("\"nodeId\":\"n\""));
        assert!(!json.contains("run_id"));
        assert!(!json.contains("node_id"));
    }

    /// ADR 0033 phase 13: a top-level `TokenDelta` carries no spawn/parent fields (both
    /// `None` → omitted from the wire), and tags/fields follow the same camelCase contract.
    #[test]
    fn token_delta_top_level_omits_spawn_and_parent_fields() {
        let event = RunEvent::TokenDelta {
            run_id: RunId::from("r"),
            node_id: NodeId::from("assistant"),
            message_id: "r:turn-0".to_owned(),
            delta: "Hel".to_owned(),
            parent_run_id: None,
            spawn_id: None,
            timestamp: "0".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"token_delta\""));
        assert!(json.contains("\"messageId\":\"r:turn-0\""));
        assert!(json.contains("\"delta\":\"Hel\""));
        // Absent spawn/parent → omitted, not `null`, so the wire stays lean.
        assert!(!json.contains("spawnId"));
        assert!(!json.contains("parentRunId"));
    }

    /// A `mapAgents` spawn's delta carries `spawnId` + `parentRunId` (camelCase), so a
    /// consumer can demultiplex interleaved sub-agent streams.
    #[test]
    fn token_delta_spawn_carries_spawn_and_parent_fields() {
        let event = RunEvent::TokenDelta {
            run_id: RunId::from("r"),
            node_id: NodeId::from("fanner"),
            message_id: "r:spawn2:turn-1".to_owned(),
            delta: "lo".to_owned(),
            parent_run_id: Some(RunId::from("r")),
            spawn_id: Some(2),
            timestamp: "0".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"spawnId\":2"));
        assert!(json.contains("\"parentRunId\":\"r\""));
    }
}
