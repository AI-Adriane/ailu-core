//! What a host reads about a run, decided by the engine (ADR 0045 D3.4): whether a replay
//! reproduced the decisions a run was attested for, and where a run stands.
//!
//! - [`verify_replay_decisions`] — the faithfulness check of replay-as-evidence (ADR 0038): the
//!   ordered `{ status, subject }` decisions of the attested chain against those a replay made.
//!   Separate from the tamper-evidence of the chain (its Ed25519 signatures): it compares strings,
//!   in order, and never touches crypto. `decidedAt`, `resolvedBy` and `approvalId` are not
//!   compared — wall-clock, human and random facts a re-execution does not reproduce.
//! - [`explain_run`] — a structured account of a run from its `GraphState` (and, optionally, its
//!   lifecycle events): its status, why and where it is suspended and what unblocks it, what
//!   failed, the names of its channels (never their values) and its last events.
//!
//! Both were TypeScript-only (`verifyReplayDecisions`, `explainRun`); their results here are the
//! ones the TypeScript SDK gave, checked against golden cases recorded from it. They work on JSON
//! values and keep JavaScript's rules where they show: `String(value)` in messages, and names
//! sorted by UTF-16 code unit, as `Array.prototype.sort` does.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use ailu_graph_runtime::{SIGNALS_KEY, SUSPEND_META_KEY};
use serde_json::{json, Map, Value};

/// Lifecycle events an explanation lists, the most recent last.
const RECENT_EVENTS: usize = 20;

/// Compare the ordered decisions of the attested chain with those of a replay. A decision is
/// `{ status, subject }`, and its `callKey` when it names its call (ADR 0051 D3): when BOTH sides
/// carry a call key they must be equal; a side without one (a record attested before ADR 0051)
/// is compared by status and subject only. Other fields are carried, not compared. A decision
/// missing on either side, or whose status, subject or call differs, is a mismatch at its index.
/// The result is `{ ok, attested, replayed, mismatches: [{ index, attested?, replayed? }] }`.
#[must_use]
pub fn verify_replay_decisions(attested: &[Value], replayed: &[Value]) -> Value {
    let mut mismatches = Vec::new();
    for index in 0..attested.len().max(replayed.len()) {
        let (a, r) = (attested.get(index), replayed.get(index));
        let same = match (a, r) {
            (Some(a), Some(r)) => {
                let same_call = match (a.get("callKey"), r.get("callKey")) {
                    (Some(attested), Some(replayed)) => attested == replayed,
                    _ => true,
                };
                a.get("status") == r.get("status")
                    && a.get("subject") == r.get("subject")
                    && same_call
            }
            _ => false,
        };
        if !same {
            let mut mismatch = Map::new();
            mismatch.insert("index".to_owned(), json!(index));
            if let Some(a) = a {
                mismatch.insert("attested".to_owned(), a.clone());
            }
            if let Some(r) = r {
                mismatch.insert("replayed".to_owned(), r.clone());
            }
            mismatches.push(Value::Object(mismatch));
        }
    }
    json!({
        "ok": mismatches.is_empty(),
        "attested": attested,
        "replayed": replayed,
        "mismatches": mismatches,
    })
}

/// JavaScript's `String(value)` for a JSON value (`undefined` when absent).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
        Some(other) => other.to_string(),
    }
}

/// `Array.prototype.sort` order: by UTF-16 code unit.
fn js_order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn channels_of(state: &Value) -> Option<&Map<String, Value>> {
    state.get("channels").and_then(Value::as_object)
}

/// The channels a caller declared: the engine's `__*` channels left out, sorted.
fn public_channels(state: &Value) -> Vec<String> {
    let mut names: Vec<String> = channels_of(state)
        .into_iter()
        .flat_map(Map::keys)
        .filter(|name| {
            name.as_str() != SUSPEND_META_KEY
                && name.as_str() != SIGNALS_KEY
                && !name.starts_with("__")
        })
        .cloned()
        .collect();
    names.sort_by(|a, b| js_order(a, b));
    names
}

/// The tools agents wait on a person to approve: the `tool:<name>` approval requests their
/// channels list, each name once, sorted.
fn pending_tools(state: &Value) -> Vec<String> {
    let names: BTreeSet<String> = channels_of(state)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(|value| value.get("approvalRequests").and_then(Value::as_array))
        .flatten()
        .filter_map(|request| request.get("subject").and_then(Value::as_str))
        .filter_map(|subject| subject.strip_prefix("tool:"))
        .map(str::to_owned)
        .collect();
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort_by(|a, b| js_order(a, b));
    names
}

/// What unblocks a suspended run.
fn next_action(
    reason: &str,
    awaiting_signal: Option<&Value>,
    wake_at: Option<&Value>,
    tools: &[String],
) -> String {
    if awaiting_signal.is_some() {
        let signal = js_string(awaiting_signal);
        return format!(
            "deliver the \"{signal}\" signal with app.signal(runId, \"{signal}\", payload)"
        );
    }
    if wake_at.is_some() {
        return format!(
            "the control-plane scheduler resumes at {}; or call app.resume(runId)",
            js_string(wake_at)
        );
    }
    if !tools.is_empty() {
        let list = tools
            .iter()
            .map(|name| Value::String(name.clone()).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "a human approves the {list} tool call, then call app.approveAndResume(runId, {{ approvedTools: [{list}], resolvedBy }})"
        );
    }
    if reason == "human-gate" || reason == "interrupt" {
        return "a human approves, then call app.resume(runId)".to_owned();
    }
    "call app.resume(runId)".to_owned()
}

/// The most recent failure in an event log: `{ node, error }` for a failed node, `{ error }` for a
/// failed run.
fn last_failure(events: &[Value]) -> Option<Map<String, Value>> {
    events.iter().rev().find_map(|event| {
        let mut failure = Map::new();
        match event.get("type").and_then(Value::as_str) {
            Some("node_failed") => {
                failure.insert("node".to_owned(), json!(js_string(event.get("nodeId"))));
            }
            Some("run_failed") => {}
            _ => return None,
        }
        if let Some(error) = event.get("error") {
            failure.insert("error".to_owned(), error.clone());
        }
        Some(failure)
    })
}

/// Explain a run from its `GraphState` and, optionally, its lifecycle events: `{ runId, status,
/// currentNode, summary, channels, recentEvents?, suspended?, failure? }`. Read-only; channel names
/// only, never their values.
#[must_use]
pub fn explain_run(state: &Value, events: Option<&[Value]>) -> Value {
    let status = js_string(state.get("status"));
    let current_node = js_string(state.get("currentNodeId"));
    let channels = public_channels(state);
    let mut explanation = Map::new();
    explanation.insert("runId".to_owned(), json!(js_string(state.get("runId"))));
    explanation.insert("status".to_owned(), json!(status));
    explanation.insert("currentNode".to_owned(), json!(current_node));
    explanation.insert("summary".to_owned(), json!(""));
    explanation.insert("channels".to_owned(), json!(channels));
    if let Some(events) = events {
        let recent: Vec<Value> = events[events.len().saturating_sub(RECENT_EVENTS)..]
            .iter()
            .map(|event| {
                let mut entry = Map::new();
                if let Some(kind) = event.get("type") {
                    entry.insert("type".to_owned(), kind.clone());
                }
                if event.get("nodeId").is_some() {
                    entry.insert("node".to_owned(), json!(js_string(event.get("nodeId"))));
                }
                Value::Object(entry)
            })
            .collect();
        explanation.insert("recentEvents".to_owned(), Value::Array(recent));
    }

    let summary = match status.as_str() {
        "suspended" => {
            let meta = channels_of(state)
                .and_then(|channels| channels.get(SUSPEND_META_KEY))
                .and_then(Value::as_object)
                .filter(|meta| meta.get("reason").is_some_and(Value::is_string));
            let reason = meta
                .and_then(|meta| meta.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("interrupt");
            let awaiting_signal = meta.and_then(|meta| meta.get("awaitingSignal"));
            let wake_at = meta.and_then(|meta| meta.get("wakeAt"));
            let action = next_action(reason, awaiting_signal, wake_at, &pending_tools(state));
            let mut suspended = Map::new();
            suspended.insert("reason".to_owned(), json!(reason));
            suspended.insert("node".to_owned(), json!(current_node));
            if let Some(signal) = awaiting_signal {
                suspended.insert("awaitingSignal".to_owned(), signal.clone());
            }
            if let Some(wake_at) = wake_at {
                suspended.insert("wakeAt".to_owned(), wake_at.clone());
            }
            suspended.insert("nextAction".to_owned(), json!(action));
            explanation.insert("suspended".to_owned(), Value::Object(suspended));
            format!("Suspended at \"{current_node}\" ({reason}). To continue: {action}.")
        }
        "failed" => match events.and_then(last_failure) {
            Some(failure) => {
                let at = failure
                    .get("node")
                    .and_then(Value::as_str)
                    .filter(|node| !node.is_empty())
                    .map(|node| format!(" at \"{node}\""))
                    .unwrap_or_default();
                let error = js_string(failure.get("error"));
                explanation.insert("failure".to_owned(), Value::Object(failure));
                format!("Failed{at}: {error}")
            }
            None => format!("Failed at \"{current_node}\"."),
        },
        "completed" => {
            let names = if channels.is_empty() {
                "(none)".to_owned()
            } else {
                channels.join(", ")
            };
            format!("Completed. Final channels: {names}.")
        }
        _ => format!("Status \"{status}\" at \"{current_node}\"."),
    };
    explanation.insert("summary".to_owned(), json!(summary));
    Value::Object(explanation)
}

/// [`verify_replay_decisions`] for the bindings: two JSON arrays of decisions in, the result out.
///
/// # Errors
///
/// When either input is not a JSON array.
pub fn verify_replay_decisions_json(
    attested_json: &str,
    replayed_json: &str,
) -> Result<String, String> {
    let attested: Vec<Value> = serde_json::from_str(attested_json)
        .map_err(|error| format!("invalid attested decisions JSON: {error}"))?;
    let replayed: Vec<Value> = serde_json::from_str(replayed_json)
        .map_err(|error| format!("invalid replayed decisions JSON: {error}"))?;
    Ok(verify_replay_decisions(&attested, &replayed).to_string())
}

/// [`explain_run`] for the bindings: a `GraphState` and, optionally, a JSON array of run events
/// in, the explanation out.
///
/// # Errors
///
/// When the state is not JSON or the events are not a JSON array.
pub fn explain_run_json(state_json: &str, events_json: Option<&str>) -> Result<String, String> {
    let state: Value =
        serde_json::from_str(state_json).map_err(|error| format!("invalid state JSON: {error}"))?;
    let events: Option<Vec<Value>> = events_json
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("invalid run events JSON: {error}"))?;
    Ok(explain_run(&state, events.as_deref()).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(status: &str, current: &str, channels: Value) -> Value {
        json!({ "runId": "run-1", "graphId": "g", "currentNodeId": current, "status": status,
                "channels": channels, "version": 1, "createdAt": "0", "updatedAt": "0" })
    }

    #[test]
    fn a_replay_that_reproduces_every_decision_in_order_is_ok() {
        let decisions = vec![
            json!({ "status": "approved", "subject": "gate:review" }),
            json!({ "status": "rejected", "subject": "tool:refund" }),
        ];
        let result = verify_replay_decisions(&decisions, &decisions);
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["mismatches"], json!([]));
    }

    #[test]
    fn a_flipped_or_missing_decision_is_a_mismatch_at_its_index() {
        let attested = vec![
            json!({ "status": "approved", "subject": "gate:review" }),
            json!({ "status": "approved", "subject": "tool:refund" }),
        ];
        let replayed = vec![json!({ "status": "rejected", "subject": "gate:review" })];
        let result = verify_replay_decisions(&attested, &replayed);
        assert_eq!(result["ok"], json!(false));
        assert_eq!(
            result["mismatches"],
            json!([
                { "index": 0, "attested": attested[0], "replayed": replayed[0] },
                { "index": 1, "attested": attested[1] }
            ])
        );
    }

    #[test]
    fn a_replay_must_request_the_same_call_when_both_sides_name_it() {
        // ADR 0051 D3: a decision that names its call (`callKey`) on both sides is reproduced only
        // by the same call; a side without one (a record attested before ADR 0051) compares by
        // subject, as before.
        let call = |key: &str| json!({ "status": "", "subject": "tool:refund", "callKey": key });
        let a = format!("refund#{}", "a".repeat(64));
        let b = format!("refund#{}", "b".repeat(64));
        assert_eq!(
            verify_replay_decisions(&[call(&a)], &[call(&a)])["ok"],
            json!(true)
        );
        let other = verify_replay_decisions(&[call(&a)], &[call(&b)]);
        assert_eq!(other["ok"], json!(false));
        assert_eq!(other["mismatches"][0]["index"], json!(0));
        let older = json!({ "status": "", "subject": "tool:refund" });
        assert_eq!(
            verify_replay_decisions(std::slice::from_ref(&older), &[call(&b)])["ok"],
            json!(true)
        );
        assert_eq!(
            verify_replay_decisions(&[call(&a)], &[older])["ok"],
            json!(true)
        );
    }

    #[test]
    fn a_run_waiting_for_a_signal_says_how_to_deliver_it() {
        let explained = explain_run(
            &state(
                "suspended",
                "wait",
                json!({ "__suspend": { "reason": "signal", "awaitingSignal": "paid" }, "amount": 1 }),
            ),
            None,
        );
        assert_eq!(explained["suspended"]["awaitingSignal"], json!("paid"));
        assert_eq!(
            explained["summary"],
            json!("Suspended at \"wait\" (signal). To continue: deliver the \"paid\" signal with app.signal(runId, \"paid\", payload).")
        );
        assert_eq!(explained["channels"], json!(["amount"]));
        assert!(explained.get("recentEvents").is_none());
    }

    #[test]
    fn a_failed_run_names_its_last_failure_and_lists_its_recent_events() {
        let events: Vec<Value> = (0..25)
            .map(|n| json!({ "type": "node_started", "nodeId": format!("n{n}"), "runId": "run-1" }))
            .chain([
                json!({ "type": "node_failed", "nodeId": "send", "error": "503", "attempt": 1 }),
                json!({ "type": "run_failed", "error": "send failed" }),
            ])
            .collect();
        let explained = explain_run(&state("failed", "send", json!({})), Some(&events));
        assert_eq!(explained["failure"], json!({ "error": "send failed" }));
        assert_eq!(explained["summary"], json!("Failed: send failed"));
        let recent = explained["recentEvents"].as_array().unwrap();
        assert_eq!(recent.len(), 20);
        assert_eq!(recent[19], json!({ "type": "run_failed" }));
    }

    #[test]
    fn names_sort_by_utf16_code_unit_as_javascript_does() {
        let explained = explain_run(
            &state(
                "completed",
                "end",
                json!({ "\u{e000}": 1, "\u{1f600}": 2, "a": 3 }),
            ),
            None,
        );
        assert_eq!(explained["channels"], json!(["a", "\u{1f600}", "\u{e000}"]));
    }

    #[test]
    fn the_json_entries_refuse_input_that_does_not_parse() {
        assert!(verify_replay_decisions_json("{}", "[]").is_err());
        assert!(explain_run_json("{", None).is_err());
        assert!(explain_run_json("{}", Some("{}")).is_err());
    }
}
