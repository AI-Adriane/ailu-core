//! Host-tool replay journal (ADR 0041 D2).
//!
//! Opening host tools to catalog runs (ADR 0041 D1) without journaling their results would break
//! replay-as-evidence: a replay would RE-EXECUTE the tools (side effects, non-determinism). This
//! module mirrors the LLM journal's doctrine ([`ailu_llm_gateway::replay_journal`]):
//!
//! - **Record**: every host-tool invocation appends `{ name, inputHash, result | error }` to a
//!   run-scoped shared log, in call order.
//! - **Replay**: host tools are NEVER re-executed. Each call is served by the recorded entry
//!   matching `(name, inputHash)` — consumed once, occurrence order for identical keys (safe
//!   under `mapAgents` fan-out concurrency, exactly like LLM request-equality matching). A call
//!   with entries present but NO match is a divergence (`tool_input_mismatch`) — the replay must
//!   fail loudly, never fall through to a live execution.
//! - **Compat**: a pre-0041 journal has no `toolResults`; replaying it serves the deterministic
//!   no-op stub instead (the E1-documented degradation), so old evidence stays verifiable.
//!
//! Only the INPUT HASH is stored (sha256 of the canonical serde_json encoding — serde_json maps
//! are BTree-ordered, so serialization is key-order canonical): the raw inputs already live in
//! the recorded LLM tool calls; the hash guards divergence without duplicating content.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One recorded host-tool invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultWire {
    /// The tool's name (as the agent called it).
    pub name: String,
    /// sha256 (hex) of the canonical JSON of the call's input.
    pub input_hash: String,
    /// The host's result — present when the call succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The host's error string — present when the call failed (replayed as the same failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// sha256 (hex) of a tool input's canonical JSON encoding.
pub fn hash_tool_input(input: &Value) -> String {
    let canonical = serde_json::to_string(input).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The replay-side log: recorded entries consumed once, matched by `(name, inputHash)`.
pub struct ToolReplayLog {
    entries: Mutex<Vec<(ToolResultWire, bool)>>,
    /// Whether the journal carried any tool entries at all (a pre-0041 journal does not —
    /// callers then degrade to the deterministic stub instead of failing).
    recorded: bool,
}

/// What the log says a replayed call should do.
pub enum ToolReplayOutcome {
    /// Serve this recorded result (or recorded failure).
    Serve(Result<Value, String>),
    /// The journal predates tool recording — degrade to the deterministic stub.
    NoJournal,
    /// Entries exist but none matches `(name, inputHash)` — the replay diverged.
    Mismatch,
}

impl ToolReplayLog {
    pub fn new(entries: Vec<ToolResultWire>) -> Self {
        let recorded = !entries.is_empty();
        Self {
            entries: Mutex::new(entries.into_iter().map(|e| (e, false)).collect()),
            recorded,
        }
    }

    /// Whether the journal recorded any host-tool results (a pre-0041 journal did not).
    pub fn has_entries(&self) -> bool {
        self.recorded
    }

    /// Take the first unconsumed entry matching `(name, inputHash)`.
    pub fn take_matching(&self, name: &str, input: &Value) -> ToolReplayOutcome {
        if !self.recorded {
            return ToolReplayOutcome::NoJournal;
        }
        let input_hash = hash_tool_input(input);
        let mut entries = self.entries.lock().expect("tool replay mutex poisoned");
        for (entry, consumed) in entries.iter_mut() {
            if !*consumed && entry.name == name && entry.input_hash == input_hash {
                *consumed = true;
                return ToolReplayOutcome::Serve(match (&entry.result, &entry.error) {
                    (_, Some(error)) => Err(error.clone()),
                    (Some(result), None) => Ok(result.clone()),
                    (None, None) => Ok(Value::Null),
                });
            }
        }
        ToolReplayOutcome::Mismatch
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(name: &str, input: &Value, result: Value) -> ToolResultWire {
        ToolResultWire {
            name: name.to_owned(),
            input_hash: hash_tool_input(input),
            result: Some(result),
            error: None,
        }
    }

    #[test]
    fn hash_is_stable_and_input_sensitive() {
        let a = json!({ "q": "gates", "k": 3 });
        let b = json!({ "k": 3, "q": "gates" });
        // serde_json maps are BTree-ordered: key order in the literal does not matter.
        assert_eq!(hash_tool_input(&a), hash_tool_input(&b));
        assert_ne!(
            hash_tool_input(&a),
            hash_tool_input(&json!({ "q": "other", "k": 3 }))
        );
    }

    #[test]
    fn serves_matching_entries_once_in_occurrence_order() {
        let input = json!({ "q": "gates" });
        let log = ToolReplayLog::new(vec![
            entry("search", &input, json!({ "hits": [1] })),
            entry("search", &input, json!({ "hits": [2] })),
        ]);
        match log.take_matching("search", &input) {
            ToolReplayOutcome::Serve(Ok(v)) => assert_eq!(v, json!({ "hits": [1] })),
            _ => panic!("expected first recorded result"),
        }
        match log.take_matching("search", &input) {
            ToolReplayOutcome::Serve(Ok(v)) => assert_eq!(v, json!({ "hits": [2] })),
            _ => panic!("expected second recorded result"),
        }
        assert!(matches!(
            log.take_matching("search", &input),
            ToolReplayOutcome::Mismatch
        ));
    }

    #[test]
    fn mismatched_name_or_input_is_a_divergence() {
        let log = ToolReplayLog::new(vec![entry("search", &json!({ "q": "a" }), json!({}))]);
        assert!(matches!(
            log.take_matching("other", &json!({ "q": "a" })),
            ToolReplayOutcome::Mismatch
        ));
        assert!(matches!(
            log.take_matching("search", &json!({ "q": "b" })),
            ToolReplayOutcome::Mismatch
        ));
    }

    #[test]
    fn an_empty_journal_degrades_to_the_stub_never_fails() {
        let log = ToolReplayLog::new(vec![]);
        assert!(matches!(
            log.take_matching("search", &json!({})),
            ToolReplayOutcome::NoJournal
        ));
    }

    #[test]
    fn a_recorded_error_replays_as_the_same_failure() {
        let input = json!({});
        let log = ToolReplayLog::new(vec![ToolResultWire {
            name: "search".to_owned(),
            input_hash: hash_tool_input(&input),
            result: None,
            error: Some("upstream 500".to_owned()),
        }]);
        match log.take_matching("search", &input) {
            ToolReplayOutcome::Serve(Err(e)) => assert_eq!(e, "upstream 500"),
            _ => panic!("expected the recorded failure"),
        }
    }

    #[test]
    fn wire_shape_is_camel_case_and_lean() {
        let w = entry("search", &json!({ "q": 1 }), json!({ "ok": true }));
        let s = serde_json::to_string(&w).unwrap();
        assert!(s.contains("\"inputHash\""));
        assert!(!s.contains("\"error\"")); // skipped when None
    }
}
