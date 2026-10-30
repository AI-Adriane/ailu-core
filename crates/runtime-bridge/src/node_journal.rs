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

#[cfg(not(test))]
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::replay_complexity::MeasuredMap as HashMap;
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
    entries: Mutex<ReplayEntries>,
    /// The first divergence met, if any: the bridge refuses the replay with it once the run ends.
    divergence: Mutex<Option<String>>,
}

// Randomized hash lookup avoids rescanning unrelated or consumed entries. Each key's queue
// preserves recorded occurrence order, including failures followed by successful retries.
type ReplayEntries = HashMap<(String, String), VecDeque<Result<Value, String>>>;

impl NodeReplayLog {
    pub fn new(entries: Vec<NodeResultWire>) -> Self {
        let mut indexed = ReplayEntries::new();
        for entry in entries {
            let outcome = match (entry.update, entry.error) {
                (_, Some(error)) => Err(error),
                (Some(update), None) => Ok(update),
                (None, None) => Ok(Value::Null),
            };
            indexed
                .entry((entry.node_id, entry.input_hash))
                .or_default()
                .push_back(outcome);
        }
        Self {
            entries: Mutex::new(indexed),
            divergence: Mutex::new(None),
        }
    }

    /// Take the first unconsumed entry matching `(node_id, hash(channels))`. A miss is recorded
    /// as the replay's divergence.
    pub fn take_matching(&self, node_id: &str, channels: &Value) -> NodeReplayOutcome {
        let input_hash = hash_node_input(channels);
        let mut entries = self.entries.lock().expect("node replay mutex poisoned");
        let outcome = entries
            .get_mut(&(node_id.to_owned(), input_hash))
            .and_then(VecDeque::pop_front);
        if let Some(outcome) = outcome {
            return NodeReplayOutcome::Serve(outcome);
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
    fn interleaved_keys_preserve_outcomes_and_first_divergence() {
        let input = json!({ "a": 1, "b": 2 });
        let other = json!({ "a": 2 });
        let log = NodeReplayLog::new(vec![
            NodeResultWire {
                error: Some("recorded failure".to_owned()),
                ..entry("send", &input, json!("error wins"))
            },
            entry("draft", &input, json!("draft result")),
            NodeResultWire {
                update: None,
                ..entry("send", &other, Value::Null)
            },
            entry("send", &input, json!("retry result")),
        ]);
        assert!(matches!(
            log.take_matching("missing", &input),
            NodeReplayOutcome::Mismatch(_)
        ));
        let first_divergence = log.divergence();
        assert!(matches!(
            log.take_matching("send", &json!({ "a": 3 })),
            NodeReplayOutcome::Mismatch(_)
        ));
        // Misses do not consume entries, and different keys can be replayed out of order.
        assert!(matches!(
            log.take_matching("send", &other),
            NodeReplayOutcome::Serve(Ok(Value::Null))
        ));
        match log.take_matching("draft", &input) {
            NodeReplayOutcome::Serve(Ok(value)) => assert_eq!(value, json!("draft result")),
            _ => panic!("expected the other node's result"),
        }
        match log.take_matching("send", &json!({ "b": 2, "a": 1 })) {
            NodeReplayOutcome::Serve(Err(error)) => assert_eq!(error, "recorded failure"),
            _ => panic!("expected the recorded error, even with an update present"),
        }
        match log.take_matching("send", &input) {
            NodeReplayOutcome::Serve(Ok(value)) => assert_eq!(value, json!("retry result")),
            _ => panic!("expected the successful retry"),
        }
        assert!(matches!(
            log.take_matching("send", &input),
            NodeReplayOutcome::Mismatch(_)
        ));
        assert_eq!(log.divergence(), first_divergence);
    }

    #[test]
    fn a_large_journal_replays_distinct_keys_in_reverse_order() {
        let count = 8_192;
        let entries = (0..count)
            .map(|i| entry(&format!("node-{}", i % 64), &json!(i / 64), json!(i)))
            .collect();
        let log = NodeReplayLog::new(entries);
        for i in (0..count).rev() {
            match log.take_matching(&format!("node-{}", i % 64), &json!(i / 64)) {
                NodeReplayOutcome::Serve(Ok(value)) => assert_eq!(value, json!(i)),
                _ => panic!("expected distinct entry {i}"),
            }
        }
        assert!(log.divergence().is_none());
    }

    #[test]
    fn lookup_work_stays_linear_for_reverse_hits_and_repeated_misses() {
        for count in [64, 256, 1_024] {
            let log = NodeReplayLog::new(
                (0..count)
                    .map(|i| entry(&format!("node-{}", i % 64), &json!(i / 64), json!(i)))
                    .collect(),
            );
            for i in (0..count).rev() {
                // Misses while the log still holds entries must not rescan the journal.
                assert!(matches!(
                    log.take_matching("missing", &Value::Null),
                    NodeReplayOutcome::Mismatch(_)
                ));
                let node_id = format!("node-{}", i % 64);
                let input = json!(i / 64);
                match log.take_matching(&node_id, &input) {
                    NodeReplayOutcome::Serve(Ok(value)) => assert_eq!(value, json!(i)),
                    _ => panic!("expected distinct entry {i}"),
                }
                assert!(matches!(
                    log.take_matching(&node_id, &input),
                    NodeReplayOutcome::Mismatch(_)
                ));
            }
            let work = log.entries.lock().unwrap().lookup_work();
            assert!(work >= 3 * count, "every lookup must be measured");
            assert!(
                work <= 4 * count,
                "{count} entries required {work} units of lookup work; full-journal scans are forbidden"
            );
        }
    }

    #[test]
    fn concurrent_duplicate_lookups_consume_each_outcome_once() {
        let count = 1_024;
        let log = NodeReplayLog::new(
            (0..count)
                .map(|i| entry("send", &Value::Null, json!(i)))
                .collect(),
        );
        let mut served = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..count / 8)
                            .map(|_| match log.take_matching("send", &Value::Null) {
                                NodeReplayOutcome::Serve(Ok(value)) => value.as_u64().unwrap(),
                                _ => panic!("expected an unconsumed outcome"),
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        served.sort_unstable();
        assert_eq!(served, (0..count).collect::<Vec<_>>());
        assert!(log.divergence().is_none());
        assert!(matches!(
            log.take_matching("send", &Value::Null),
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
