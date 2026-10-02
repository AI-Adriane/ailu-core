//! Host-node replay journal and effect key (ADR 0045 D1).
//!
//! A host node (an id in `EngineSpec.hostNodeIds`) runs the host's code as a graph step. Unlike a
//! pure step, it may write to the outside world — send a message, file a draft — so a replay must
//! never call it again. This module mirrors the host-tool journal ([`crate::tool_journal`]):
//!
//! - **Record**: every host-node execution appends `{ nodeId, inputHash, update | error }` to a
//!   run-scoped log, in execution order.
//! - **Replay**: a host node is NEVER called. Each execution is served by the first unconsumed
//!   entry matching `(nodeId, inputHash)` — occurrence order for identical keys, so a node retried
//!   after a recorded failure replays the failure, then the success. No match is a divergence
//!   (`node_input_mismatch`): the node fails and the whole replay is refused — never a live call.
//! - **Compat**: a journal recorded before ADR 0045 has no `nodeResults` key at all (a recorded
//!   run without host nodes carries an empty list). Replaying an old journal keeps calling host
//!   nodes, as before — a builder graph's pure steps are re-derived — so old evidence stays
//!   verifiable.
//!
//! The input is the channel state the host received, hashed and never stored: the channels
//! already live in the run's checkpoints.
//!
//! The **effect key** is what makes an external effect at-most-once on the host side: sha256 of
//! `(runId, nodeId, state version at the node's entry)`. A step retried from the same checkpoint —
//! the worker is at-least-once, a node may declare retries — gets the same key; the next
//! execution of the same node in a loop gets another, since the version moved.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::tool_journal::hash_tool_input;

/// One recorded host-node execution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeResultWire {
    /// The node's id (global — a subgraph node keeps its own id).
    pub node_id: String,
    /// sha256 (hex) of the canonical JSON of the channels the node received.
    pub input_hash: String,
    /// The channel update the host returned — present when the step succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Value>,
    /// The host's error — present when the step failed (replayed as the same failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// sha256 (hex) of the channels a host node received.
pub fn hash_node_input(channels: &Value) -> String {
    hash_tool_input(channels)
}

/// The effect key of one host-node execution: sha256 (hex) of `[runId, nodeId, version]` as
/// canonical JSON — an array, so no id can collide with another by containing a separator.
pub fn effect_key(run_id: &str, node_id: &str, version: u64) -> String {
    let canonical = json!([run_id, node_id, version]).to_string();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// What the log says a replayed host node should do.
pub enum NodeReplayOutcome {
    /// Serve this recorded update (or recorded failure).
    Serve(Result<Value, String>),
    /// No unconsumed entry matches `(nodeId, inputHash)` — the replay diverged.
    Mismatch(String),
}

/// The replay-side log: recorded entries consumed once, matched by `(nodeId, inputHash)`.
pub struct NodeReplayLog {
    entries: Mutex<Vec<(NodeResultWire, bool)>>,
    /// The first divergence met, if any: the bridge refuses the replay with it once the run ends.
    divergence: Mutex<Option<String>>,
}

impl NodeReplayLog {
    pub fn new(entries: Vec<NodeResultWire>) -> Self {
        Self {
            entries: Mutex::new(entries.into_iter().map(|entry| (entry, false)).collect()),
            divergence: Mutex::new(None),
        }
    }

    /// Take the first unconsumed entry matching `(node_id, hash(channels))`. A miss is recorded
    /// as the replay's divergence.
    pub fn take_matching(&self, node_id: &str, channels: &Value) -> NodeReplayOutcome {
        let input_hash = hash_node_input(channels);
        let mut entries = self.entries.lock().expect("node replay mutex poisoned");
        for (entry, consumed) in entries.iter_mut() {
            if !*consumed && entry.node_id == node_id && entry.input_hash == input_hash {
                *consumed = true;
                return NodeReplayOutcome::Serve(match (&entry.update, &entry.error) {
                    (_, Some(error)) => Err(error.clone()),
                    (Some(update), None) => Ok(update.clone()),
                    (None, None) => Ok(Value::Null),
                });
            }
        }
        let message = format!(
            "node_input_mismatch: replayed host node '{node_id}' has no matching recorded result — the replay diverged from the journal"
        );
        let mut divergence = self.divergence.lock().expect("node replay mutex poisoned");
        if divergence.is_none() {
            *divergence = Some(message.clone());
        }
        NodeReplayOutcome::Mismatch(message)
    }

    /// The first divergence this replay met, if any.
    pub fn divergence(&self) -> Option<String> {
        self.divergence
            .lock()
            .expect("node replay mutex poisoned")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(node_id: &str, channels: &Value, update: Value) -> NodeResultWire {
        NodeResultWire {
            node_id: node_id.to_owned(),
            input_hash: hash_node_input(channels),
            update: Some(update),
            error: None,
        }
    }

    #[test]
    fn the_effect_key_is_stable_and_moves_with_run_node_and_version() {
        let key = effect_key("run-1", "send", 3);
        assert_eq!(key, effect_key("run-1", "send", 3));
        assert_eq!(key.len(), 64);
        assert_ne!(key, effect_key("run-2", "send", 3));
        assert_ne!(key, effect_key("run-1", "draft", 3));
        assert_ne!(key, effect_key("run-1", "send", 4));
        // An id holding what a naive separator would be cannot collide with another pair.
        assert_ne!(effect_key("a:b", "c", 1), effect_key("a", "b:c", 1));
    }

    #[test]
    fn serves_matching_entries_once_in_occurrence_order() {
        let channels = json!({ "proposal": "p-1" });
        let log = NodeReplayLog::new(vec![
            NodeResultWire {
                node_id: "send".to_owned(),
                input_hash: hash_node_input(&channels),
                update: None,
                error: Some("upstream 503".to_owned()),
            },
            entry("send", &channels, json!({ "receipt": "ts-1" })),
        ]);
        match log.take_matching("send", &channels) {
            NodeReplayOutcome::Serve(Err(error)) => assert_eq!(error, "upstream 503"),
            _ => panic!("expected the recorded failure first"),
        }
        match log.take_matching("send", &channels) {
            NodeReplayOutcome::Serve(Ok(update)) => {
                assert_eq!(update, json!({ "receipt": "ts-1" }))
            }
            _ => panic!("expected the recorded success second"),
        }
        assert!(log.divergence().is_none());
        assert!(matches!(
            log.take_matching("send", &channels),
            NodeReplayOutcome::Mismatch(_)
        ));
        assert!(log
            .divergence()
            .is_some_and(|message| message.starts_with("node_input_mismatch")));
    }

    #[test]
    fn another_node_or_another_input_is_a_divergence() {
        let log = NodeReplayLog::new(vec![entry("send", &json!({ "to": "a" }), json!({}))]);
        assert!(matches!(
            log.take_matching("draft", &json!({ "to": "a" })),
            NodeReplayOutcome::Mismatch(_)
        ));
        assert!(matches!(
            log.take_matching("send", &json!({ "to": "b" })),
            NodeReplayOutcome::Mismatch(_)
        ));
    }

    #[test]
    fn an_empty_journal_serves_nothing() {
        // A run recorded after ADR 0045 that executed no host node: any host node on replay is a
        // divergence, never a live call.
        let log = NodeReplayLog::new(vec![]);
        assert!(matches!(
            log.take_matching("send", &json!({})),
            NodeReplayOutcome::Mismatch(_)
        ));
    }

    #[test]
    fn wire_shape_is_camel_case_and_lean() {
        let wire = entry("send", &json!({ "q": 1 }), json!({ "ok": true }));
        let text = serde_json::to_string(&wire).unwrap();
        assert!(text.contains("\"nodeId\""));
        assert!(text.contains("\"inputHash\""));
        assert!(!text.contains("\"error\""));
    }
}
