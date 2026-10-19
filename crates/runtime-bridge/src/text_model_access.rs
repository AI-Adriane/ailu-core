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
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Host {
        checkpoints: Mutex<Vec<Value>>,
    }
    #[async_trait]
    impl HostCallbacks for Host {
        async fn on_node(&self, _: Value) -> BridgeResult<String> {
            Ok("{}".into())
        }
        fn on_condition(&self, _: Value) -> BridgeResult<bool> {
            Ok(true)
        }
        fn on_event(&self, _: String) {}
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
        let result: Value = serde_json::from_str(
            &run(denied.to_string(), Arc::new(Host::default()), Entry::Start)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(result.to_string().contains("text_model_access_denied"));
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
