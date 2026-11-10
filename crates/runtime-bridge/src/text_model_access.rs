//! Additive state/checkpoint view for the text-model snapshot. GraphState and its reducers remain
//! unchanged: policy is execution metadata, never an agent-writable channel.

use ailu_llm_gateway::TextModelAccessSnapshot;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{spec::EngineSpec, BridgeResult, Entry};

const INVALID: &str = "text_model_access_invalid";

pub fn validate_policy_hash(snapshot: &TextModelAccessSnapshot) -> BridgeResult<()> {
    snapshot.validate().map_err(|_| INVALID.to_owned())?;
    let mut models: Vec<_> = snapshot.allowed_models.iter().collect();
    models.sort_by(|a, b| {
        ailu_llm_gateway::provider_slug(a.provider)
            .cmp(ailu_llm_gateway::provider_slug(b.provider))
            .then_with(|| a.model_id.encode_utf16().cmp(b.model_id.encode_utf16()))
    });
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Definition<'a> {
        schema_version: u32,
        catalog_revision: &'a str,
        enabled_models: Vec<&'a ailu_llm_gateway::TextModelIdentity>,
    }
    let definition = Definition {
        schema_version: 1,
        catalog_revision: &snapshot.catalog_revision,
        enabled_models: models,
    };
    let bytes = serde_json::to_vec(&definition).map_err(|_| INVALID.to_owned())?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != snapshot.policy_hash {
        return Err(INVALID.to_owned());
    }
    Ok(())
}

/// Missing or replaced binding is refused on every governed resume, including approve/signal/replay.
/// The authenticated host is responsible for retaining the entire envelope in its durable store.
pub fn validate_entry(raw: &Value, spec: &EngineSpec, entry: &Entry) -> BridgeResult<()> {
    if raw.get("textModelAccess").is_some_and(Value::is_null) {
        return Err(INVALID.to_owned());
    }
    let snapshot = spec.text_model_access.as_ref();
    if let Some(policy) = snapshot {
        validate_policy_hash(policy)?;
        let run_id = match entry {
            Entry::Start => spec.run_id.as_deref(),
            _ => spec.state.as_ref().map(|state| state.run_id.0.as_str()),
        };
        policy
            .bind(spec.tenant_id.as_deref(), run_id)
            .map_err(|_| INVALID.to_owned())?;
        if spec.run_id.as_deref().is_some_and(|id| id != policy.run_id) {
            return Err(INVALID.to_owned());
        }
    }
    let held = raw
        .get("state")
        .and_then(|state| state.get("textModelAccess"));
    if !matches!(entry, Entry::Start) || held.is_some() {
        let held = held
            .map(|value| {
                serde_json::from_value::<TextModelAccessSnapshot>(value.clone())
                    .map_err(|_| INVALID.to_owned())
            })
            .transpose()?;
        if held.as_ref() != snapshot {
            return Err("text_model_access_snapshot_mismatch".to_owned());
        }
    }
    Ok(())
}

pub fn bind_state(
    value: &mut Value,
    snapshot: Option<&TextModelAccessSnapshot>,
) -> BridgeResult<()> {
    if let Some(snapshot) = snapshot {
        let object = value.as_object_mut().ok_or_else(|| INVALID.to_owned())?;
        object.insert(
            "textModelAccess".into(),
            serde_json::to_value(snapshot).map_err(|_| INVALID.to_owned())?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{run, HostCallbacks};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Host {
        checkpoints: Mutex<Vec<Value>>,
        events: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl HostCallbacks for Host {
        async fn on_node(&self, _: Value) -> BridgeResult<String> {
            Ok("{}".into())
        }
        fn on_condition(&self, _: Value) -> BridgeResult<bool> {
            Ok(true)
        }
        fn on_event(&self, wire: String) {
            self.events.lock().unwrap().push(wire);
        }
        async fn on_checkpoint(&self, wire: String) -> BridgeResult<()> {
            self.checkpoints
                .lock()
                .unwrap()
                .push(serde_json::from_str(&wire).unwrap());
            Ok(())
        }
    }
    fn snapshot(models: Vec<Value>) -> Value {
        let definition =
            json!({"schemaVersion":1,"catalogRevision":"catalog-a","enabledModels":models});
        // Explicit canonical property order, independently matching the host's JSON.stringify contract.
        let canonical = format!(
            "{{\"schemaVersion\":1,\"catalogRevision\":\"catalog-a\",\"enabledModels\":{}}}",
            serde_json::to_string(&definition["enabledModels"]).unwrap()
        );
        let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        json!({"schemaVersion":1,"tenantId":"tenant-a","runId":"run-a","policyId":"organization-text-models",
            "policyVersion":1,"catalogRevision":"catalog-a","policyHash":digest,"allowedModels":models})
    }
    fn policy() -> Value {
        // Use a typed serialization to keep provider/modelId canonical field order in the hash.
        let mut p: TextModelAccessSnapshot = serde_json::from_value(snapshot(vec![])).unwrap();
        p.allowed_models = vec![ailu_llm_gateway::TextModelIdentity {
            provider: ailu_llm_gateway::LlmProvider::Mock,
            model_id: "mock".into(),
        }];
        p.policy_hash = format!("{:x}", Sha256::digest(b"{\"schemaVersion\":1,\"catalogRevision\":\"catalog-a\",\"enabledModels\":[{\"provider\":\"mock\",\"modelId\":\"mock\"}]}"));
        serde_json::to_value(p).unwrap()
    }
    fn spec() -> Value {
        json!({"runId":"run-a","tenantId":"tenant-a","textModelAccess":policy(),"hostCheckpointer":true,
            "graph":{"id":"g","version":"1","name":"g","nodes":[{"id":"a","type":"agent","label":"a"}],"edges":[],"entryNodeId":"a",
                "channels":{"answer":{"type":"string","reducer":"replace","default":""}}},
            "agents":{"a":{"provider":"mock","model":"mock","maxIterations":1,"outputChannel":"answer"}}})
    }

    #[tokio::test]
    async fn checkpoint_and_outcome_bind_the_same_snapshot_outside_agent_channels() {
        let host = Arc::new(Host::default());
        let wire = spec();
        let result: Value = serde_json::from_str(
            &run(wire.to_string(), host.clone(), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["state"]["textModelAccess"], wire["textModelAccess"]);
        assert!(result["state"]["channels"].get("textModelAccess").is_none());
        let checkpoints = host.checkpoints.lock().unwrap();
        assert!(!checkpoints.is_empty());
        for checkpoint in checkpoints.iter() {
            assert_eq!(
                checkpoint["graphState"]["textModelAccess"],
                wire["textModelAccess"]
            );
        }
    }

    #[tokio::test]
    async fn resume_missing_replaced_or_cross_scope_snapshot_is_refused() {
        let original = spec();
        let result: Value = serde_json::from_str(
            &run(
                original.to_string(),
                Arc::new(Host::default()),
                Entry::Start,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        for mutation in [
            "missing",
            "changed",
            "tenant",
            "run",
            "state-missing",
            "null",
        ] {
            let mut wire = original.clone();
            wire["state"] = result["state"].clone();
            match mutation {
                "missing" => {
                    wire.as_object_mut().unwrap().remove("textModelAccess");
                }
                "changed" => {
                    wire["textModelAccess"] = snapshot(vec![]);
                }
                "tenant" => wire["tenantId"] = "tenant-b".into(),
                "run" => wire["runId"] = "run-b".into(),
                "state-missing" => {
                    wire["state"]
                        .as_object_mut()
                        .unwrap()
                        .remove("textModelAccess");
                }
                _ => wire["textModelAccess"] = Value::Null,
            }
            assert!(
                run(wire.to_string(), Arc::new(Host::default()), Entry::Resume)
                    .await
                    .unwrap_err()
                    .starts_with("text_model_access_"),
                "{mutation}"
            );
        }
    }

    #[tokio::test]
    async fn legacy_without_policy_stays_legacy_and_deny_all_does_not_call_a_model() {
        let mut wire = spec();
        wire.as_object_mut().unwrap().remove("textModelAccess");
        let legacy: Value = serde_json::from_str(
            &run(wire.to_string(), Arc::new(Host::default()), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(legacy["state"].get("textModelAccess").is_none());
        assert_eq!(legacy["status"], "completed");
        let mut denied = spec();
        denied["textModelAccess"] = snapshot(vec![]);
        let host = Arc::new(Host::default());
        let result: Value = serde_json::from_str(
            &run(denied.to_string(), host.clone(), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        // A refusal is a policy decision: it fails the node and so the run, and the host sees why.
        assert_eq!(result["status"], "failed", "{result}");
        let events = host.events.lock().unwrap().join("\n");
        assert!(events.contains("text_model_access_denied"), "{events}");
    }

    #[tokio::test]
    async fn a_custom_endpoint_is_refused_under_a_policy() {
        // The allowlist names a provider and a model, not an endpoint: an endpoint override
        // must not slip through. Nothing listens on the port; the refusal comes first.
        for (key, value) in [
            ("baseUrl", "http://127.0.0.1:9/v1"),
            ("apiKeyEnv", "SOME_KEY_ENV"),
        ] {
            let mut wire = spec();
            wire["agents"]["a"] = json!({
                "provider": "openai", "model": "gpt-4o", "maxIterations": 1, "outputChannel": "answer",
                key: value
            });
            let allowed: TextModelAccessSnapshot = serde_json::from_value(policy()).unwrap();
            let mut allowed = allowed;
            allowed.allowed_models = vec![ailu_llm_gateway::TextModelIdentity {
                provider: ailu_llm_gateway::LlmProvider::Openai,
                model_id: "gpt-4o".into(),
            }];
            allowed.policy_hash = format!(
                "{:x}",
                Sha256::digest(b"{\"schemaVersion\":1,\"catalogRevision\":\"catalog-a\",\"enabledModels\":[{\"provider\":\"openai\",\"modelId\":\"gpt-4o\"}]}")
            );
            wire["textModelAccess"] = serde_json::to_value(allowed).unwrap();
            let error = run(wire.to_string(), Arc::new(Host::default()), Entry::Start)
                .await
                .unwrap_err();
            assert!(
                error.ends_with("text_model_access_invalid"),
                "{key}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn a_custom_endpoint_is_still_allowed_without_a_policy() {
        let mut wire = spec();
        wire.as_object_mut().unwrap().remove("textModelAccess");
        wire["agents"]["a"]["baseUrl"] = "http://127.0.0.1:9/v1".into();
        // Without a policy the legacy path is unchanged: the mock agent ignores the endpoint
        // only if it is not an OpenAI-compatible one, so the run must not fail with the policy
        // error.
        let outcome = run(wire.to_string(), Arc::new(Host::default()), Entry::Start).await;
        if let Err(error) = outcome {
            assert_ne!(error, "text_model_access_invalid");
        }
    }

    #[tokio::test]
    async fn a_standalone_completion_with_an_endpoint_is_refused_under_a_policy() {
        let request = json!({
            "provider": "openai", "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "baseUrl": "http://127.0.0.1:9/v1",
            "tenantId": "tenant-a", "runId": "run-a",
            "textModelAccess": policy()
        });
        let error = crate::llm_complete_json(&request.to_string(), "{}")
            .await
            .unwrap_err();
        assert_eq!(error, "text_model_access_invalid");
    }

    #[tokio::test]
    async fn a_refusal_fails_the_run_instead_of_writing_an_error_into_a_channel() {
        let mut denied = spec();
        denied["textModelAccess"] = snapshot(vec![]);
        let result: Value = serde_json::from_str(
            &run(denied.to_string(), Arc::new(Host::default()), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["status"], "failed", "{result}");
        assert!(
            result["state"]["channels"]["answer"] != json!({"error": "text_model_access_denied"})
        );
    }

    #[tokio::test]
    async fn invalid_snapshots_are_refused_at_start() {
        let base = spec();
        let mut cases: Vec<(&str, Value)> = Vec::new();
        for (name, field, value) in [
            ("policy version zero", "policyVersion", json!(0)),
            (
                "policy version past 2^53",
                "policyVersion",
                json!(9_007_199_254_740_992_u64),
            ),
            ("empty tenant", "tenantId", json!("")),
            ("blank policy id", "policyId", json!("  ")),
            ("empty run", "runId", json!("")),
        ] {
            let mut wire = base.clone();
            wire["textModelAccess"][field] = value;
            cases.push((name, wire));
        }
        let mut empty_model = base.clone();
        empty_model["textModelAccess"]["allowedModels"] =
            json!([{"provider": "mock", "modelId": " "}]);
        cases.push(("empty model id", empty_model));
        let mut duplicate = base.clone();
        duplicate["textModelAccess"]["allowedModels"] = json!([
            {"provider": "mock", "modelId": "mock"},
            {"provider": "mock", "modelId": "mock"}
        ]);
        cases.push(("duplicate model", duplicate));
        let mut null = base.clone();
        null["textModelAccess"] = Value::Null;
        cases.push(("null snapshot", null));
        for (name, wire) in cases {
            let error = run(wire.to_string(), Arc::new(Host::default()), Entry::Start)
                .await
                .unwrap_err();
            assert!(error.starts_with("text_model_access_"), "{name}: {error}");
        }
    }

    /// `mapAgents` under a policy: every spawn is checked; a refusal fails the node and the run.
    fn map_spec(allowed: bool) -> Value {
        let mut wire = spec();
        wire["graph"]["nodes"] = json!([{"id": "a", "type": "agent", "label": "a"}]);
        wire["graph"]["channels"] = json!({
            "items": {"type": "json", "reducer": "replace", "default": ["one", "two"]},
            "answer": {"type": "json", "reducer": "replace"}
        });
        wire.as_object_mut().unwrap().remove("agents");
        wire["mapAgents"] = json!({"a": {
            "overChannel": "items", "joinAt": "answer",
            "agent": {"provider": "mock", "model": "mock", "maxIterations": 1}
        }});
        if !allowed {
            wire["textModelAccess"] = snapshot(vec![]);
        }
        wire
    }

    #[tokio::test]
    async fn map_agents_under_a_policy_runs_when_allowed_and_fails_when_refused() {
        let allowed: Value = serde_json::from_str(
            &run(
                map_spec(true).to_string(),
                Arc::new(Host::default()),
                Entry::Start,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(allowed["status"], "completed", "{allowed}");
        let host = Arc::new(Host::default());
        let refused: Value = serde_json::from_str(
            &run(map_spec(false).to_string(), host.clone(), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(refused["status"], "failed", "{refused}");
        assert!(host
            .events
            .lock()
            .unwrap()
            .join("\n")
            .contains("text_model_access_denied"));
    }

    const PROVIDER_OUTPUT: &str = "counted-provider-output";

    /// A local provider that counts the connections it answers (one per request: it closes
    /// each). The engine reaches it as Ollama, whose address the operator sets, not the graph.
    fn counting_provider() -> (String, Arc<AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                counted.fetch_add(1, Ordering::SeqCst);
                // Read the headers, then the body they announce.
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 8192];
                    let n = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&request);
                    let complete = text.find("\r\n\r\n").is_some_and(|end| {
                        let size = text[..end].lines().find_map(|line| {
                            let line = line.to_ascii_lowercase();
                            line.strip_prefix("content-length:")?.trim().parse().ok()
                        });
                        request.len() >= end + 4 + size.unwrap_or(0)
                    });
                    if n == 0 || complete {
                        break;
                    }
                }
                let body = json!({"choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": format!("FINAL: {PROVIDER_OUTPUT}")}}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (base_url, calls)
    }

    #[test]
    fn a_subgraph_agent_under_a_policy_fails_the_run_when_refused() {
        // The hash is over `provider` then `modelId`, the host's order (see `policy`).
        let mut counted = snapshot(vec![]);
        counted["allowedModels"] = json!([{"provider": "ollama", "modelId": "counted"}]);
        counted["policyHash"] = json!(format!("{:x}", Sha256::digest(
            b"{\"schemaVersion\":1,\"catalogRevision\":\"catalog-a\",\"enabledModels\":[{\"provider\":\"ollama\",\"modelId\":\"counted\"}]}"
        )));
        let mut wire = spec();
        let child = json!({
            "id": "child", "version": "1", "name": "child",
            "channels": {"answer": {"type": "string", "reducer": "replace", "default": ""}},
            "nodes": [{"id": "ca", "type": "agent", "label": "ca"}],
            "edges": [], "entryNodeId": "ca"
        });
        wire["graph"]["nodes"] = json!([{
            "id": "a", "type": "subgraph", "label": "a", "subgraphId": "child",
            "inputMapping": {}, "outputMapping": {"answer": "answer"}
        }]);
        wire["subgraphs"] = json!([child]);
        wire["agents"] = json!({"ca": {
            "provider": "ollama", "model": "counted", "maxIterations": 1, "outputChannel": "answer"
        }});
        let (base_url, calls) = counting_provider();
        let (allowed_host, refused_host) = (Arc::new(Host::default()), Arc::new(Host::default()));

        // Run both under the env lock (a plain test: the guard is not held across an await),
        // and assert after releasing it.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let guard = crate::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = ["AILU_USE_OLLAMA", "AILU_OLLAMA_BASE_URL"].map(|key| std::env::var(key).ok());
        std::env::set_var("AILU_USE_OLLAMA", "1");
        std::env::set_var("AILU_OLLAMA_BASE_URL", &base_url);
        wire["textModelAccess"] = counted;
        let allowed = runtime.block_on(run(wire.to_string(), allowed_host.clone(), Entry::Start));
        let calls_when_allowed = calls.load(Ordering::SeqCst);
        wire["textModelAccess"] = snapshot(vec![]);
        let refused = runtime.block_on(run(wire.to_string(), refused_host.clone(), Entry::Start));
        let calls_after_refusal = calls.load(Ordering::SeqCst);
        let [use_ollama, ollama_url] = saved;
        crate::tests::restore_env("AILU_USE_OLLAMA", use_ollama);
        crate::tests::restore_env("AILU_OLLAMA_BASE_URL", ollama_url);
        drop(guard);

        // Control: allowed, the provider is called once and its output reaches the checkpoints.
        let allowed: Value = serde_json::from_str(&allowed.unwrap()).unwrap();
        assert_eq!(allowed["status"], "completed", "{allowed}");
        assert_eq!(calls_when_allowed, 1);
        let checkpoints = |host: &Host| json!(*host.checkpoints.lock().unwrap()).to_string();
        assert!(checkpoints(&allowed_host).contains(PROVIDER_OUTPUT));

        // Refused: the parent error is generic; the cause is the child's own events, and only
        // the refusal. The provider is not called and no checkpoint holds a model output.
        let error = refused.unwrap_err();
        assert!(error.contains("subgraph 'child' failed"), "{error}");
        assert_eq!(
            calls_after_refusal, 1,
            "the refused run called the provider"
        );
        let events: Vec<Value> = (refused_host.events.lock().unwrap().iter())
            .map(|wire| serde_json::from_str(wire).unwrap())
            .collect();
        let failures: Vec<(&str, &str, &str)> = (events.iter())
            .filter(|event| event["type"].as_str().unwrap().ends_with("_failed"))
            .map(|event| {
                let field = |name: &str| event[name].as_str().unwrap_or("");
                (field("type"), field("nodeId"), field("error"))
            })
            .collect();
        assert_eq!(
            failures,
            [
                ("node_failed", "ca", "text_model_access_denied"),
                ("run_failed", "", "text_model_access_denied")
            ]
        );
        assert!(events.iter().all(|event| event["type"] != "node_completed"));
        let written = checkpoints(&refused_host);
        assert!(!written.contains(PROVIDER_OUTPUT), "{written}");
        assert!(!written.contains("text_model_access_denied"), "{written}");
    }

    /// agent `a` -> human gate `g` -> agent `b`, all on the allowed mock model.
    fn gated_spec() -> Value {
        let mut wire = spec();
        wire["graph"]["nodes"] = json!([
            {"id": "a", "type": "agent", "label": "a"},
            {"id": "g", "type": "human-gate", "label": "g"},
            {"id": "b", "type": "agent", "label": "b"}
        ]);
        wire["graph"]["edges"] = json!([
            {"id": "e1", "from": "a", "to": "g", "type": "default"},
            {"id": "e2", "from": "g", "to": "b", "type": "default"}
        ]);
        wire["agents"] = json!({
            "a": {"provider": "mock", "model": "mock", "maxIterations": 1, "outputChannel": "answer"},
            "b": {"provider": "mock", "model": "mock", "maxIterations": 1, "outputChannel": "answer"}
        });
        wire
    }

    async fn suspended_state() -> Value {
        let started: Value = serde_json::from_str(
            &run(
                gated_spec().to_string(),
                Arc::new(Host::default()),
                Entry::Start,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(started["status"], "suspended", "{started}");
        started["state"].clone()
    }

    #[tokio::test]
    async fn a_governed_run_resumes_and_approves_past_its_gate() {
        let state = suspended_state().await;
        assert_eq!(state["textModelAccess"], gated_spec()["textModelAccess"]);
        for entry in [Entry::Resume, Entry::Approve] {
            let mut wire = gated_spec();
            wire["state"] = state.clone();
            let outcome: Value = serde_json::from_str(
                &run(wire.to_string(), Arc::new(Host::default()), entry)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(outcome["status"], "completed", "{outcome}");
            // The binding survives the resume, outside the channels.
            assert_eq!(
                outcome["state"]["textModelAccess"],
                gated_spec()["textModelAccess"]
            );
        }
    }

    /// A host whose node `w` waits for the signal `go`.
    struct SignalHost;
    #[async_trait]
    impl HostCallbacks for SignalHost {
        async fn on_node(&self, _: Value) -> BridgeResult<String> {
            Ok(r#"{"__waitForSignal":"go"}"#.into())
        }
        fn on_condition(&self, _: Value) -> BridgeResult<bool> {
            Ok(true)
        }
        fn on_event(&self, _: String) {}
        async fn on_checkpoint(&self, _: String) -> BridgeResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_governed_signal_advances_a_waiting_run_and_keeps_the_binding() {
        let mut wire = spec();
        wire["graph"]["nodes"] = json!([
            {"id": "w", "type": "action", "label": "w"},
            {"id": "a", "type": "agent", "label": "a"}
        ]);
        wire["graph"]["edges"] = json!([{"id": "e1", "from": "w", "to": "a", "type": "default"}]);
        wire["graph"]["entryNodeId"] = "w".into();
        wire["hostNodeIds"] = json!(["w"]);
        let started: Value = serde_json::from_str(
            &run(wire.to_string(), Arc::new(SignalHost), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(started["status"], "suspended", "{started}");
        wire["state"] = started["state"].clone();
        let outcome: Value = serde_json::from_str(
            &run(
                wire.to_string(),
                Arc::new(SignalHost),
                Entry::Signal {
                    name: "go".into(),
                    payload: json!({"ok": true}),
                },
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["status"], "completed", "{outcome}");
        assert_eq!(outcome["state"]["textModelAccess"], wire["textModelAccess"]);
    }

    #[tokio::test]
    async fn a_state_of_another_run_is_refused_on_every_governed_entry() {
        let state = suspended_state().await;
        for entry in [
            Entry::Resume,
            Entry::Approve,
            Entry::Signal {
                name: "go".into(),
                payload: Value::Null,
            },
            Entry::Replay {
                checkpoint_id: "not-read".into(),
            },
        ] {
            let mut wire = gated_spec();
            wire["state"] = state.clone();
            // The state says it belongs to another run than the snapshot binds.
            wire["state"]["runId"] = "run-b".into();
            let error = run(wire.to_string(), Arc::new(Host::default()), entry)
                .await
                .unwrap_err();
            assert!(error.starts_with("text_model_access_"), "{error}");
        }
    }

    #[tokio::test]
    async fn replay_checks_the_original_binding_before_reading_any_journal() {
        let original = spec();
        let result: Value = serde_json::from_str(
            &run(
                original.to_string(),
                Arc::new(Host::default()),
                Entry::Start,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        for supplied in [None, Some(snapshot(vec![]))] {
            let mut wire = original.clone();
            wire["state"] = result["state"].clone();
            if let Some(policy) = supplied {
                wire["textModelAccess"] = policy;
            } else {
                wire.as_object_mut().unwrap().remove("textModelAccess");
            }
            let error = run(
                wire.to_string(),
                Arc::new(Host::default()),
                Entry::Replay {
                    checkpoint_id: "not-read".into(),
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error, "text_model_access_snapshot_mismatch");
        }
    }

    #[test]
    fn canonical_hash_is_independent_of_allowlist_order_and_detects_tampering() {
        let p: TextModelAccessSnapshot = serde_json::from_value(policy()).unwrap();
        assert!(validate_policy_hash(&p).is_ok());
        let mut bad = p;
        bad.allowed_models.clear();
        assert!(validate_policy_hash(&bad).is_err());
    }
}
