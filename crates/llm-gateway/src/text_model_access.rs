//! Immutable, admission-scoped text-model authorization. The trusted host supplies the snapshot;
//! this is an authorization boundary, not a credential resolver or a policy-signature verifier.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{LlmError, LlmProvider};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextModelIdentity {
    pub provider: LlmProvider,
    pub model_id: String,
}

/// Version one of the additive JSON contract. Empty `allowed_models` denies every text call.
/// Hash/catalog/version describe the host's policy artifact; they do not authenticate the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextModelAccessSnapshot {
    pub schema_version: u32,
    pub tenant_id: String,
    pub run_id: String,
    pub policy_id: String,
    pub policy_version: u64,
    pub catalog_revision: String,
    pub policy_hash: String,
    pub allowed_models: Vec<TextModelIdentity>,
}

impl TextModelAccessSnapshot {
    pub fn validate(&self) -> Result<(), LlmError> {
        let ids = [
            &self.tenant_id,
            &self.run_id,
            &self.policy_id,
            &self.catalog_revision,
        ];
        let mut seen = HashSet::new();
        if self.schema_version != 1
            || self.policy_version == 0
            || self.policy_version > 9_007_199_254_740_991
            || ids.iter().any(|id| id.trim().is_empty())
            || self.policy_hash.len() != 64
            || !self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self
                .allowed_models
                .iter()
                .any(|model| model.model_id.trim().is_empty() || !seen.insert(model))
        {
            return Err(LlmError::TextModelAccessInvalid);
        }
        Ok(())
    }

    pub fn bind(&self, tenant_id: Option<&str>, run_id: Option<&str>) -> Result<(), LlmError> {
        self.validate()?;
        if tenant_id != Some(self.tenant_id.as_str()) || run_id != Some(self.run_id.as_str()) {
            return Err(LlmError::TextModelAccessInvalid);
        }
        Ok(())
    }
}

/// Compiled immutable exact allowlist, checked only after the provider adapter resolves its model.
pub(crate) struct TextModelAccessPolicy {
    allowed: HashSet<TextModelIdentity>,
}

impl TextModelAccessPolicy {
    pub(crate) fn new(snapshot: &TextModelAccessSnapshot) -> Result<Self, LlmError> {
        snapshot.validate()?;
        Ok(Self {
            allowed: snapshot.allowed_models.iter().cloned().collect(),
        })
    }
    pub(crate) fn check(&self, provider: LlmProvider, model: &str) -> Result<(), LlmError> {
        if !self.allowed.contains(&TextModelIdentity {
            provider,
            model_id: model.to_owned(),
        }) {
            return Err(LlmError::TextModelAccessDenied);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefaultLlmGateway, LlmGateway, LlmRequest, LlmResponse, LlmUsage, MockAdapter};

    fn snapshot() -> TextModelAccessSnapshot {
        TextModelAccessSnapshot {
            schema_version: 1,
            tenant_id: "tenant-a".into(),
            run_id: "run-a".into(),
            policy_id: "organization-text-models".into(),
            policy_version: 1,
            catalog_revision: "catalog-1".into(),
            policy_hash: "a".repeat(64),
            allowed_models: vec![TextModelIdentity {
                provider: LlmProvider::Anthropic,
                model_id: "approved".into(),
            }],
        }
    }
    fn request(provider: LlmProvider, model: &str) -> LlmRequest {
        serde_json::from_value(
            serde_json::json!({ "provider": provider, "model": model, "messages": [] }),
        )
        .unwrap()
    }
    fn inner() -> DefaultLlmGateway {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![LlmResponse {
                web_search: None,
                content: "permitted".into(),
                tool_calls: None,
                stop_reason: None,
                usage: LlmUsage::default(),
                model: "approved".into(),
                provider: LlmProvider::Anthropic,
                content_blocks: None,
            }],
        )));
        gateway
    }

    #[tokio::test]
    async fn exact_identity_is_checked_before_adapter_dispatch_and_stream_deltas() {
        let gateway = inner().with_text_model_access(&snapshot()).unwrap();
        for (provider, model) in [
            (LlmProvider::Anthropic, "denied"),
            (LlmProvider::Openai, "approved"),
            (LlmProvider::Anthropic, ""),
        ] {
            assert_eq!(
                gateway.complete(request(provider, model)).await,
                Err(LlmError::TextModelAccessDenied)
            );
            let emitted = std::sync::Mutex::new(false);
            assert_eq!(
                gateway
                    .stream(request(provider, model), &|_| *emitted.lock().unwrap() =
                        true)
                    .await,
                Err(LlmError::TextModelAccessDenied)
            );
            assert!(!*emitted.lock().unwrap());
        }
        assert_eq!(
            gateway
                .complete(request(LlmProvider::Anthropic, "approved"))
                .await
                .unwrap()
                .content,
            "permitted"
        );
    }

    #[tokio::test]
    async fn empty_policy_denies_all_and_does_not_fall_back_to_registered_model() {
        let mut policy = snapshot();
        policy.allowed_models.clear();
        let gateway = inner().with_text_model_access(&policy).unwrap();
        assert_eq!(
            gateway
                .complete(request(LlmProvider::Anthropic, "approved"))
                .await,
            Err(LlmError::TextModelAccessDenied)
        );
    }

    #[tokio::test]
    async fn real_adapter_defaults_are_checked_after_resolution_for_complete_and_stream() {
        use crate::{
            AnthropicAdapter, AnthropicCreateParams, AnthropicPort, AnthropicRawResponse,
            GeminiAdapter, GeminiPort, GeminiRawResponse, LlmProviderAdapter,
            OpenAiCompatibleAdapter, OpenAiCompatiblePort,
        };
        use async_trait::async_trait;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Port(Arc<AtomicUsize>);
        #[async_trait]
        impl AnthropicPort for Port {
            async fn create(
                &self,
                _: AnthropicCreateParams,
            ) -> Result<AnthropicRawResponse, LlmError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(LlmError::Provider("test port called".into()))
            }
        }
        #[async_trait]
        impl GeminiPort for Port {
            async fn generate(
                &self,
                _: String,
                _: serde_json::Value,
            ) -> Result<GeminiRawResponse, LlmError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(LlmError::Provider("test port called".into()))
            }
        }
        #[async_trait]
        impl OpenAiCompatiblePort for Port {
            async fn send(&self, _: serde_json::Value) -> Result<serde_json::Value, LlmError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(LlmError::Provider("test port called".into()))
            }
        }
        for provider in [
            LlmProvider::Anthropic,
            LlmProvider::Google,
            LlmProvider::Openai,
        ] {
            let called = Arc::new(AtomicUsize::new(0));
            let adapter: Box<dyn LlmProviderAdapter> = match provider {
                LlmProvider::Anthropic => Box::new(AnthropicAdapter::with_default_model(
                    Box::new(Port(called.clone())),
                    "claude-actual",
                )),
                LlmProvider::Google => Box::new(GeminiAdapter::with_default_model(
                    Box::new(Port(called.clone())),
                    "gemini-actual",
                )),
                _ => Box::new(OpenAiCompatibleAdapter::with_provider(
                    Box::new(Port(called.clone())),
                    provider,
                    "actual-openai",
                )),
            };
            let mut policy = snapshot();
            policy.allowed_models = vec![TextModelIdentity {
                provider,
                model_id: "react-agent".into(),
            }];
            let mut gateway = DefaultLlmGateway::new()
                .with_text_model_access(&policy)
                .unwrap();
            gateway.register_adapter(adapter);
            assert_eq!(
                gateway.complete(request(provider, "react-agent")).await,
                Err(LlmError::TextModelAccessDenied)
            );
            assert_eq!(
                gateway
                    .stream(request(provider, "react-agent"), &|_| panic!(
                        "denied token"
                    ))
                    .await,
                Err(LlmError::TextModelAccessDenied)
            );
            assert_eq!(called.load(Ordering::SeqCst), 0);
            policy.allowed_models[0].model_id = match provider {
                LlmProvider::Anthropic => "claude-actual",
                LlmProvider::Google => "gemini-actual",
                _ => "actual-openai",
            }
            .into();
            gateway = gateway.with_text_model_access(&policy).unwrap();
            assert_eq!(
                gateway.complete(request(provider, "react-agent")).await,
                Err(LlmError::Provider("test port called".into()))
            );
            assert_eq!(called.load(Ordering::SeqCst), 1);
            assert_eq!(
                gateway.complete(request(provider, "")).await,
                Err(LlmError::Provider("test port called".into()))
            );
            assert_eq!(
                gateway
                    .stream(request(provider, ""), &|_| panic!("no fake token"))
                    .await,
                Err(LlmError::Provider("test port called".into()))
            );
            assert_eq!(called.load(Ordering::SeqCst), 3);
        }
    }

    #[test]
    fn snapshot_is_strict_scoped_and_duplicate_free() {
        let policy = snapshot();
        assert!(policy.bind(Some("tenant-a"), Some("run-a")).is_ok());
        assert!(policy.bind(Some("tenant-b"), Some("run-a")).is_err());
        assert!(policy.bind(Some("tenant-a"), Some("run-b")).is_err());
        let mut duplicate = policy.clone();
        duplicate
            .allowed_models
            .push(duplicate.allowed_models[0].clone());
        assert!(duplicate.validate().is_err());
        let mut malformed = policy.clone();
        malformed.schema_version = 2;
        assert!(malformed.validate().is_err());
        let mut wire = serde_json::to_value(&policy).unwrap();
        wire["unknown"] = true.into();
        assert!(serde_json::from_value::<TextModelAccessSnapshot>(wire).is_err());
    }
}
