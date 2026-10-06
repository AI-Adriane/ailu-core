//! The callback-capable graph runner for Python (ADR 0045 D2.2).
//!
//! It drives the same [`ailu_runtime_bridge::run`] as the TypeScript (N-API) and C ABI SDKs, over
//! the same `EngineSpec`: host nodes and host tools (`on_node`), named conditions
//! (`on_condition`), lifecycle events (`on_event`) and cooperative cancellation
//! (`is_cancelled`). Host nodes are journaled in record mode and served on replay by the bridge
//! itself (ADR 0045 D1), so Python gets that guarantee without code of its own.
//!
//! The host's functions are plain Rust closures over JSON text here — so this layer is tested
//! without an interpreter; the pyo3 layer only wraps Python callables into them.
//!
//! A run drives a fresh current-thread tokio runtime on the calling thread: every callback runs
//! on that thread, in order (the pyo3 layer releases the GIL around the run and takes it back in
//! each callback).

use std::sync::Arc;

use ailu_runtime_bridge::{BridgeResult, Entry, HostCallbacks};
use async_trait::async_trait;
use serde_json::Value;

/// A host node or host tool call: the `on_node` payload as JSON in, the result as JSON out.
pub type NodeFn = Box<dyn Fn(String) -> Result<String, String> + Send + Sync>;
/// A named condition: `{ name, state }` as JSON in, the branch decision out.
pub type ConditionFn = Box<dyn Fn(String) -> Result<bool, String> + Send + Sync>;
/// A run lifecycle event, as JSON.
pub type EventFn = Box<dyn Fn(String) + Send + Sync>;
/// Polled at every node boundary: `true` stops the run there (ADR 0044).
pub type CancelFn = Box<dyn Fn() -> bool + Send + Sync>;
/// Keeps one checkpoint (a serialized `Checkpoint`) in the host's store, before the run goes on
/// (ADR 0049 D1). `Err` stops the run.
pub type CheckpointFn = Box<dyn Fn(String) -> Result<(), String> + Send + Sync>;

/// The host side of one run. A missing `on_node` or `on_condition` fails what needs it, loudly —
/// never an empty update or a `false` that would silently route the run elsewhere.
#[derive(Default)]
pub struct HostFns {
    pub on_node: Option<NodeFn>,
    pub on_condition: Option<ConditionFn>,
    pub on_event: Option<EventFn>,
    pub is_cancelled: Option<CancelFn>,
    pub on_checkpoint: Option<CheckpointFn>,
}

#[async_trait]
impl HostCallbacks for HostFns {
    async fn on_node(&self, payload: Value) -> BridgeResult<String> {
        match &self.on_node {
            Some(on_node) => on_node(payload.to_string()),
            None => Err("no on_node function was given for a host node or host tool".to_owned()),
        }
    }

    fn on_condition(&self, payload: Value) -> BridgeResult<bool> {
        match &self.on_condition {
            Some(on_condition) => on_condition(payload.to_string()),
            None => Err("no on_condition function was given for a named condition".to_owned()),
        }
    }

    fn on_event(&self, payload_json: String) {
        if let Some(on_event) = &self.on_event {
            on_event(payload_json);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled
            .as_ref()
            .is_some_and(|is_cancelled| is_cancelled())
    }

    async fn on_checkpoint(&self, checkpoint_json: String) -> BridgeResult<()> {
        match &self.on_checkpoint {
            Some(on_checkpoint) => on_checkpoint(checkpoint_json),
            None => Ok(()),
        }
    }
}

/// Run one entry of the bridge over `spec_json` and return the `RunOutcome` JSON.
pub fn run(spec_json: &str, host: HostFns, entry: Entry) -> Result<String, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the engine runtime: {error}"))?
        .block_on(ailu_runtime_bridge::run(
            spec_json.to_owned(),
            Arc::new(host),
            entry,
        ))
}

/// The entry that delivers signal `name` with its payload (JSON text), then resumes.
pub fn signal_entry(name: &str, payload_json: &str) -> Result<Entry, String> {
    let payload = serde_json::from_str(payload_json)
        .map_err(|error| format!("invalid signal payload JSON: {error}"))?;
    Ok(Entry::Signal {
        name: name.to_owned(),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    /// One host node, `send`, reading `proposal` and writing `receipt`.
    fn spec(extra: Value) -> String {
        let mut spec = json!({
            "graph": {
                "id": "g",
                "version": "0.0.0",
                "name": "g",
                "channels": {
                    "proposal": { "type": "string", "reducer": "replace" },
                    "receipt": { "type": "string", "reducer": "replace" }
                },
                "nodes": [{ "id": "send", "type": "action", "label": "send" }],
                "edges": [],
                "entryNodeId": "send"
            },
            "runId": "run-1",
            "hostNodeIds": ["send"],
            "initialData": { "proposal": "p-1" }
        });
        if let (Some(spec), Value::Object(extra)) = (spec.as_object_mut(), extra) {
            spec.extend(extra);
        }
        spec.to_string()
    }

    fn outcome(json: &str) -> Value {
        serde_json::from_str(json).expect("outcome is JSON")
    }

    #[test]
    fn runs_a_host_node_through_the_bridge_with_its_effect_key() {
        let payloads = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::clone(&payloads);
        let host = HostFns {
            on_node: Some(Box::new(move |payload| {
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_str(&payload).unwrap());
                Ok("{\"receipt\":\"r-1\"}".to_owned())
            })),
            ..HostFns::default()
        };
        let result = outcome(&run(&spec(json!({})), host, Entry::Start).expect("run completes"));
        assert_eq!(result["status"], json!("completed"));
        assert_eq!(result["state"]["channels"]["receipt"], json!("r-1"));
        let payloads = payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["kind"], json!("node"));
        assert_eq!(payloads[0]["state"]["proposal"], json!("p-1"));
        assert_eq!(payloads[0]["effectKey"].as_str().map(str::len), Some(64));
    }

    #[test]
    fn a_host_error_fails_the_node_and_a_missing_function_too() {
        let failing = HostFns {
            on_node: Some(Box::new(|_| Err("ValueError: slack 503".to_owned()))),
            ..HostFns::default()
        };
        let result = outcome(&run(&spec(json!({})), failing, Entry::Start).unwrap());
        assert_eq!(result["status"], json!("failed"));

        let result = outcome(&run(&spec(json!({})), HostFns::default(), Entry::Start).unwrap());
        assert_eq!(result["status"], json!("failed"));
    }

    #[test]
    fn events_are_forwarded_and_cancellation_is_polled() {
        let events = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&events);
        let host = HostFns {
            on_node: Some(Box::new(|_| Ok("{}".to_owned()))),
            on_event: Some(Box::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
            is_cancelled: Some(Box::new(|| true)),
            ..HostFns::default()
        };
        let result = outcome(&run(&spec(json!({})), host, Entry::Start).unwrap());
        // Cancelled at the first boundary: the step never ran, the stop is an event.
        assert_eq!(result["status"], json!("cancelled"));
        assert!(events.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn a_spec_that_does_not_parse_is_an_error() {
        let error = run("{", HostFns::default(), Entry::Start).expect_err("invalid spec");
        assert!(error.contains("invalid engine spec JSON"), "{error}");
    }

    #[test]
    fn a_signal_entry_carries_its_parsed_payload() {
        match signal_entry("paid", "{\"amount\":42}").unwrap() {
            Entry::Signal { name, payload } => {
                assert_eq!(name, "paid");
                assert_eq!(payload, json!({ "amount": 42 }));
            }
            other => panic!("expected a signal entry, got {other:?}"),
        }
        assert!(signal_entry("paid", "not json").is_err());
    }

    #[test]
    fn a_host_store_keeps_each_checkpoint_and_one_that_is_down_stops_the_run() {
        // ADR 0049 D1: asked through `hostCheckpointer`, the store gets the entry checkpoint and
        // the completed one — and a store that cannot keep one fails the run call.
        let kept = Arc::new(Mutex::new(Vec::<Value>::new()));
        let store = Arc::clone(&kept);
        let host = HostFns {
            on_node: Some(Box::new(|_| Ok("{\"receipt\":\"r-1\"}".to_owned()))),
            on_checkpoint: Some(Box::new(move |checkpoint| {
                store
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&checkpoint).unwrap());
                Ok(())
            })),
            ..HostFns::default()
        };
        let result = outcome(
            &run(
                &spec(json!({ "hostCheckpointer": true })),
                host,
                Entry::Start,
            )
            .expect("run completes"),
        );
        assert_eq!(result["status"], json!("completed"));
        let kept = kept.lock().unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1]["graphState"]["status"], json!("completed"));
        assert_eq!(kept[1]["id"], result["state"]["checkpointId"]);

        let down = HostFns {
            on_node: Some(Box::new(|_| Ok("{}".to_owned()))),
            on_checkpoint: Some(Box::new(|_| Err("OSError: store down".to_owned()))),
            ..HostFns::default()
        };
        let error = run(
            &spec(json!({ "hostCheckpointer": true })),
            down,
            Entry::Start,
        )
        .expect_err("a store that is down stops the run");
        assert!(error.contains("store down"), "{error}");
    }
}
