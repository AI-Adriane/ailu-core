//! PII redaction seam (ADR 0008, phase 2 — native).
//!
//! The engine ships only the trait, a no-op default, and a generic HTTP client. The
//! heavy detection (Presidio/GLiNER, or the control plane's own redactor) lives behind
//! `PII_REDACTOR_URL`. [`RedactingGateway`] wraps any [`LlmGateway`] so every request is
//! scrubbed before it reaches a provider — closing the intermediate-message gap that
//! input/output redaction at the control plane cannot see (tool observations and prior
//! turns fed back into the loop).
//!
//! Hydration of the FINAL answer stays at the control plane (it owns the per-run vault
//! that maps placeholders back to the user's own values); the gateway seam only redacts,
//! so [`PiiRedactor::hydrate_response`] defaults to identity.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::gateway::LlmGateway;
use crate::types::{ContentBlock, LlmRequest, LlmResponse};

/// Scrubs PII from an outbound request before it reaches a provider.
#[async_trait]
pub trait PiiRedactor: Send + Sync {
    /// Return a copy of `request` with personal data removed from its system prompt and
    /// message contents. Returns `Err(LlmError::PiiBlocked)` when a `block`-level policy
    /// detects personal data — the run then fails instead of silently continuing (the
    /// gate stops; full human-resume mid-loop is a later refinement).
    async fn redact_request(&self, request: LlmRequest) -> Result<LlmRequest, LlmError>;

    /// Restore the user's own values in a response. Defaults to identity — the control
    /// plane re-hydrates the final answer from the per-run vault.
    async fn hydrate_response(&self, response: LlmResponse) -> LlmResponse {
        response
    }
}

/// Default: pass everything through unchanged (no redaction). This is what the OSS
/// engine runs with unless a deployment configures a real redactor.
pub struct NoopPiiRedactor;

#[async_trait]
impl PiiRedactor for NoopPiiRedactor {
    async fn redact_request(&self, request: LlmRequest) -> Result<LlmRequest, LlmError> {
        Ok(request)
    }
}

/// Wraps any [`LlmGateway`] so every `complete()` redacts the request before the
/// provider sees it, then hydrates the response after. Compose it around the bare
/// gateway: `RedactingGateway::new(inner, redactor)`.
pub struct RedactingGateway {
    inner: Arc<dyn LlmGateway>,
    redactor: Arc<dyn PiiRedactor>,
}

impl RedactingGateway {
    pub fn new(inner: Arc<dyn LlmGateway>, redactor: Arc<dyn PiiRedactor>) -> Self {
        Self { inner, redactor }
    }
}

#[async_trait]
impl LlmGateway for RedactingGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let redacted = self.redactor.redact_request(request).await?;
        let response = self.inner.complete(redacted).await?;
        Ok(self.redactor.hydrate_response(response).await)
    }
}

#[derive(Serialize)]
struct RedactBatchRequest {
    texts: Vec<String>,
}

#[derive(Deserialize)]
struct RedactBatchResponse {
    texts: Vec<String>,
    /// True when a `block`-level policy matched — the seam then fails the call.
    #[serde(default)]
    blocked: bool,
}

/// Calls an external redaction service over HTTP. Configure with `AILU_PII_REDACTOR_URL`
/// (the control plane's batch endpoint under approach A, or a Presidio/GLiNER adapter). The
/// wire contract is deliberately tiny: `POST { "texts": [...] } -> { "texts": [...] }`,
/// same length and order. A distinct var from the control plane's own `PII_REDACTOR_URL`
/// (which points at a Presidio `/detect` service), so the two never collide.
///
/// On a service failure (transport error, or a response that does not answer every text) it
/// logs to stderr and passes the text through unchanged (fail-open) by default: the hard block
/// lives at the control plane's input gate; this seam is defense-in-depth for intermediate
/// messages, so a flaky redaction service must not abort an otherwise-valid run. Deployments
/// where unredacted text must never leave set `AILU_PII_REDACTOR_FAIL_CLOSED=1`: a failure
/// then fails the call instead.
pub struct HttpPiiRedactor {
    url: String,
    token: Option<String>,
    client: reqwest::Client,
    fail_closed: bool,
}

impl HttpPiiRedactor {
    pub fn new(url: String, token: Option<String>) -> Self {
        Self {
            url,
            token,
            client: crate::http::http_client(),
            fail_closed: false,
        }
    }

    /// Fail the call (instead of passing text through) when the redaction service fails.
    pub fn with_fail_closed(mut self, fail_closed: bool) -> Self {
        self.fail_closed = fail_closed;
        self
    }

    /// Build from env: `AILU_PII_REDACTOR_URL` (required) + `AILU_PII_REDACTOR_TOKEN`
    /// (optional bearer) + `AILU_PII_REDACTOR_FAIL_CLOSED` (`1`/`true`). Returns `None` when
    /// the URL is unset/empty, so the caller skips wrapping and the engine runs with no redaction.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("AILU_PII_REDACTOR_URL")
            .ok()
            .filter(|value| !value.is_empty())?;
        let token = std::env::var("AILU_PII_REDACTOR_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        let fail_closed = matches!(
            std::env::var("AILU_PII_REDACTOR_FAIL_CLOSED").as_deref(),
            Ok("1") | Ok("true")
        );
        Some(Self::new(url, token).with_fail_closed(fail_closed))
    }

    async fn redact_texts(
        &self,
        texts: Vec<String>,
    ) -> Result<RedactBatchResponse, reqwest::Error> {
        let mut builder = self.client.post(&self.url);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        builder
            .json(&RedactBatchRequest { texts })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}

#[async_trait]
impl PiiRedactor for HttpPiiRedactor {
    async fn redact_request(&self, mut request: LlmRequest) -> Result<LlmRequest, LlmError> {
        // Collect system + every message content, redact as one batch, write back in the
        // same order. Guard each write so a short/garbled response never drops content.
        // ADR 0030: when a message carries content blocks, the adapter sends the TEXT BLOCKS
        // (not `content`) — so they must be scrubbed too, or block text bypasses redaction.
        // Traversal order (system, then per-message: each Text block OR `content`) is mirrored
        // exactly in the write-back below. Image/audio/file blocks are binary to the text
        // redactor (a documented media-PII gap — see ADR 0030).
        let texts = redactable_texts(&request);
        if texts.is_empty() {
            return Ok(request);
        }
        let expected = texts.len();

        let failure = match self.redact_texts(texts).await {
            Ok(response) => {
                // A `block`-level policy matched → fail the call (fail-closed): block is an
                // explicit owner choice to STOP, not to silently scrub-and-continue.
                if response.blocked {
                    return Err(LlmError::PiiBlocked(
                        "personal data detected in an intermediate message".to_owned(),
                    ));
                }
                // The write-back is positional: a response that does not answer every text
                // cannot be mapped back safely (a dropped middle entry would shift every later
                // text onto the wrong message), so it is a service failure, not a partial result.
                if response.texts.len() == expected {
                    write_back_redacted(&mut request, response.texts);
                    return Ok(request);
                }
                format!(
                    "redaction service answered {} of {expected} texts",
                    response.texts.len()
                )
            }
            Err(error) => error.to_string(),
        };
        if self.fail_closed {
            return Err(LlmError::PiiBlocked(format!(
                "PII redaction unavailable and AILU_PII_REDACTOR_FAIL_CLOSED is set: {failure}"
            )));
        }
        // Fail-open (the default): the hard block lives at the control-plane input gate; a flaky
        // redaction service must not abort an otherwise-valid run.
        eprintln!("[pii] redaction service error, passing text through: {failure}");
        Ok(request)
    }
}

/// Every text a request sends, in a fixed order: the system prompt, then per message its
/// `content` followed by its text blocks. `content` is included even when blocks are present:
/// it stays the text fallback (and the leading text part for some providers), so it is sent too.
fn redactable_texts(request: &LlmRequest) -> Vec<String> {
    let mut texts: Vec<String> = Vec::with_capacity(request.messages.len() + 1);
    if let Some(system) = &request.system {
        texts.push(system.clone());
    }
    for message in &request.messages {
        texts.push(message.content.clone());
        for block in message.content_blocks.iter().flatten() {
            if let ContentBlock::Text { text } = block {
                texts.push(text.clone());
            }
        }
    }
    texts
}

/// Write redacted texts back in exactly the [`redactable_texts`] order.
fn write_back_redacted(request: &mut LlmRequest, redacted: Vec<String>) {
    let mut next = redacted.into_iter();
    if request.system.is_some() {
        if let Some(value) = next.next() {
            request.system = Some(value);
        }
    }
    for message in request.messages.iter_mut() {
        if let Some(value) = next.next() {
            message.content = value;
        }
        for block in message.content_blocks.iter_mut().flatten() {
            if let ContentBlock::Text { text } = block {
                if let Some(value) = next.next() {
                    *text = value;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::DefaultLlmGateway;
    use crate::mock::MockAdapter;
    use crate::types::{LlmMessage, LlmProvider, LlmUsage};

    fn request_with(messages: Vec<LlmMessage>, system: Option<&str>) -> LlmRequest {
        LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "claude".to_owned(),
            messages,
            system: system.map(str::to_owned),
            tools: None,
            max_tokens: None,
            temperature: None,
            run_id: None,
            response_format: None,
        }
    }

    fn response(content: &str) -> LlmResponse {
        LlmResponse {
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("end_turn".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    /// A redactor that uppercases content, to prove the wrapper threads request and
    /// response through it without any network.
    struct UpperRedactor;

    #[async_trait]
    impl PiiRedactor for UpperRedactor {
        async fn redact_request(&self, mut request: LlmRequest) -> Result<LlmRequest, LlmError> {
            if let Some(system) = request.system.take() {
                request.system = Some(system.to_uppercase());
            }
            for message in request.messages.iter_mut() {
                message.content = message.content.to_uppercase();
            }
            Ok(request)
        }
    }

    /// A redactor that always blocks, to prove `complete()` propagates the block as an error.
    struct BlockingRedactor;

    #[async_trait]
    impl PiiRedactor for BlockingRedactor {
        async fn redact_request(&self, _request: LlmRequest) -> Result<LlmRequest, LlmError> {
            Err(LlmError::PiiBlocked("test".to_owned()))
        }
    }

    #[test]
    fn redactable_texts_cover_the_text_fallback_and_blocks_in_write_back_order() {
        let mut with_blocks = LlmMessage::text("user", "fallback alice@example.com");
        with_blocks.content_blocks = Some(vec![ContentBlock::Text {
            text: "block alice@example.com".to_owned(),
        }]);
        let mut request = request_with(
            vec![
                LlmMessage::text("user", "plain alice@example.com"),
                with_blocks,
            ],
            Some("system"),
        );
        let texts = redactable_texts(&request);
        assert_eq!(
            texts,
            vec![
                "system",
                "plain alice@example.com",
                "fallback alice@example.com",
                "block alice@example.com"
            ]
        );
        let redacted = texts
            .iter()
            .map(|text| text.replace("alice@example.com", "<EMAIL>"))
            .collect();
        write_back_redacted(&mut request, redacted);
        assert!(!serde_json::to_string(&request)
            .unwrap()
            .contains("alice@example.com"));
    }

    #[tokio::test]
    async fn fail_closed_redactor_refuses_to_pass_text_through_when_the_service_is_down() {
        // Port 9 (discard) on localhost: the connection is refused immediately.
        let redactor = HttpPiiRedactor::new("http://127.0.0.1:9/redact".to_owned(), None)
            .with_fail_closed(true);
        let error = redactor
            .redact_request(request_with(vec![LlmMessage::text("user", "hi")], None))
            .await
            .unwrap_err();
        assert!(matches!(error, LlmError::PiiBlocked(_)), "{error:?}");

        let fail_open = HttpPiiRedactor::new("http://127.0.0.1:9/redact".to_owned(), None);
        assert!(fail_open
            .redact_request(request_with(vec![LlmMessage::text("user", "hi")], None))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn noop_redactor_passes_through() {
        let redactor = NoopPiiRedactor;
        let req = request_with(vec![], Some("hello user@example.com"));
        let out = redactor.redact_request(req.clone()).await.unwrap();
        assert_eq!(out.system, req.system);
    }

    #[tokio::test]
    async fn redacting_gateway_propagates_a_block() {
        let mut inner = DefaultLlmGateway::new();
        inner.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![response("ok")],
        )));
        let gateway = RedactingGateway::new(Arc::new(inner), Arc::new(BlockingRedactor));
        let result = gateway.complete(request_with(vec![], Some("x"))).await;
        assert!(matches!(result, Err(LlmError::PiiBlocked(_))));
    }

    #[tokio::test]
    async fn redacting_gateway_redacts_before_complete() {
        let mut inner = DefaultLlmGateway::new();
        inner.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![response("ok")],
        )));
        let gateway = RedactingGateway::new(Arc::new(inner), Arc::new(UpperRedactor));

        let req = request_with(vec![LlmMessage::text("user", "secret")], Some("be nice"));
        // The mock ignores content but the call must succeed through the wrapper.
        let out = gateway.complete(req).await.unwrap();
        assert_eq!(out.content, "ok");
    }
}
