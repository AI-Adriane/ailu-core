//! LLM gateway errors.

use crate::types::LlmProvider;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LlmError {
    #[error("no adapter registered for provider '{0:?}'")]
    ProviderNotFound(LlmProvider),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("blocked by PII policy: {0}")]
    PiiBlocked(String),
    /// ADR 0029: the model's output failed JSON-Schema validation after the bounded retry
    /// budget. Surfaced at the node sink as channel data (the gateway-error-as-data
    /// convention), never a panic.
    #[error("structured output did not match the schema: {0}")]
    StructuredOutputInvalid(String),
    /// ADR 0030: a multimodal media reference could not be resolved to bytes (e.g. a missing
    /// artifact), or inline media exceeded the size cap.
    #[error("media resolution failed: {0}")]
    MediaResolution(String),
    /// ADR 0032: a secret/credential was detected in an outbound request under the opt-in
    /// `block` policy (`AILU_SECRETS_POLICY=block`). Surfaced at the node sink as channel
    /// data, never a panic. (The default policy masks-and-continues instead.)
    #[error("blocked by secrets policy: {0}")]
    SecretsBlocked(String),
    /// ADR 0038 (replay-as-evidence): a replay's recorded LLM journal has no matching
    /// response for an outbound request — a journal gap. Surfaced as an error so a replay
    /// can NEVER silently fall through to a live provider call (which would defeat the
    /// "re-feed the original decisions" guarantee).
    #[error("replay journal has no recorded response for this request: {0}")]
    ReplayJournalMiss(String),
}
