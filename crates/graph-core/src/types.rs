//! The graph data model: node/edge/channel/state types. Field names serialize to
//! the same camelCase wire shape as the TS model, so definitions are interchangeable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{EdgeId, GraphId, NodeId, RunId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeType {
    Action,
    Agent,
    Tool,
    HumanGate,
    Subgraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeType {
    Default,
    Conditional,
    /// ADR 0076 — routes from a node to a designated handler once its `retry_policy` is
    /// exhausted, instead of failing the whole run. Absent (the common case): failure
    /// behavior is unchanged. Mirrors the TS `EDGE_TYPES` addition byte-for-byte
    /// (`"error"` on the wire).
    Error,
}

/// ADR 0076 — coarse classification of why a node's handler failed, carried on
/// `NodeFailed`/`NodeErrorRouted` events so a governed error path can react differently to
/// a transient failure vs a permanent one. A handler opts in via
/// `NodeOutput::failure_with_category`; absent, it defaults to `Unknown`. Mirrors the TS
/// `FailureCategory` (kebab-case on the wire: `"transient" | "permanent" | "unknown"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureCategory {
    Transient,
    Permanent,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GraphStatus {
    Idle,
    Running,
    Suspended,
    Completed,
    Failed,
    /// Terminal: the run was cancelled cooperatively at a node BOUNDARY (ADR 0044) — the
    /// embedder's cancellation seam returned true between two nodes, so the run stopped
    /// before scheduling the next one. Distinct from `Failed` (nothing went wrong; a human
    /// or a circuit breaker asked for the stop) and from `Suspended` (not waiting for a
    /// decision — it will never advance on its own). The last checkpoint is authoritative,
    /// so a cancelled run stays fully resumable and replayable from it.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelReducer {
    Replace,
    Append,
    Merge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDefinition {
    #[serde(rename = "type")]
    pub channel_type: String,
    pub reducer: ChannelReducer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// ADR 0032 phase 10: never emit this channel's value in run events / logs. Masked with a
    /// sentinel at every event-emission site, but still CHECKPOINTED IN FULL (durable/resumable)
    /// — durability ≠ observability. Additive + serde-default, so existing graphs deserialize
    /// unchanged (and a `false` value stays off the wire).
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_log: bool,
}

/// `skip_serializing_if` helper: keep a `false` bool off the wire (serde passes `&bool`).
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(rename = "maxAttempts")]
    pub max_attempts: u32,
    #[serde(rename = "backoffMs")]
    pub backoff_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanOut {
    pub parallel_to: Vec<NodeId>,
    pub join_at: NodeId,
}

/// ADR 0042 D2/D3 (product ADR 0068 — child workflows): dynamic N-child subgraph fan-out. Present
/// on a `NodeType::Subgraph` node ALONGSIDE its existing `subgraph_id` (never duplicated here) —
/// `over_channel` is read as a JSON array at execution time, one child spawn per item, run
/// concurrently; results land in `join_at` as a JSON array, input order preserved. Absent (the
/// common case): the node runs its subgraph exactly once, unchanged (`execute_subgraph`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSubgraph {
    pub over_channel: String,
    pub join_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefinition {
    pub id: NodeId,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_id: Option<GraphId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mapping: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mapping: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_out: Option<FanOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_subgraph: Option<MapSubgraph>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDefinition {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    #[serde(rename = "type")]
    pub edge_type: EdgeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphDefinition {
    pub id: GraphId,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursion_limit: Option<u32>,
    pub channels: BTreeMap<String, ChannelDefinition>,
    pub nodes: Vec<NodeDefinition>,
    pub edges: Vec<EdgeDefinition>,
    pub entry_node_id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphState {
    pub run_id: RunId,
    pub graph_id: GraphId,
    pub current_node_id: NodeId,
    pub status: GraphStatus,
    pub channels: BTreeMap<String, serde_json::Value>,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_serializes_to_kebab_case() {
        assert_eq!(
            serde_json::to_string(&NodeType::HumanGate).unwrap(),
            "\"human-gate\""
        );
        let parsed: NodeType = serde_json::from_str("\"agent\"").unwrap();
        assert_eq!(parsed, NodeType::Agent);
    }

    #[test]
    fn node_uses_type_and_camel_case_keys() {
        let node = NodeDefinition {
            id: NodeId::from("review"),
            node_type: NodeType::HumanGate,
            label: "Review".to_owned(),
            subgraph_id: None,
            input_mapping: None,
            output_mapping: None,
            fan_out: None,
            map_subgraph: None,
            retry_policy: Some(RetryPolicy {
                max_attempts: 2,
                backoff_ms: 100,
            }),
            metadata: None,
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"type\":\"human-gate\""));
        assert!(json.contains("\"retryPolicy\":{\"maxAttempts\":2,\"backoffMs\":100}"));
        // Round-trips back to an identical value.
        let back: NodeDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, node);
    }
}
