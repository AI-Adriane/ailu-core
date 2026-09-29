//! A mock adapter that replays scripted responses — for offline runs and tests.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::error::LlmError;
use crate::gateway::{LlmProviderAdapter, TokenSink};
use crate::types::{LlmProvider, LlmRequest, LlmResponse, WebSearchOutcome};

pub struct MockAdapter {
    provider: LlmProvider,
    responses: Vec<LlmResponse>,
    /// Optional per-call delta scripts (ADR 0033 phase 13): `stream_scripts[i]` is the
    /// sequence of token deltas `stream()` emits for the i-th call. Empty (the default)
    /// → `stream()` emits the whole `responses[i].content` as one delta (chunk-once),
    /// so existing `MockAdapter::new(..)` call sites stream without change.
    stream_scripts: Vec<Vec<String>>,
    index: AtomicUsize,
}

impl MockAdapter {
    pub fn new(provider: LlmProvider, responses: Vec<LlmResponse>) -> Self {
        MockAdapter {
            provider,
            responses,
            stream_scripts: Vec::new(),
            index: AtomicUsize::new(0),
        }
    }

    /// Attach per-call token-delta scripts so `stream()` emits multiple chunks (ADR 0033).
    /// `scripts[i]` is replayed for the i-th call; absent / empty entries fall back to
    /// chunk-once. The returned [`LlmResponse`] is always `responses[i]` — a test that
    /// wants the byte-identical guarantee should make `scripts[i].concat()` equal
    /// `responses[i].content`.
    pub fn with_stream_scripts(mut self, scripts: Vec<Vec<String>>) -> Self {
        self.stream_scripts = scripts;
        self
    }
}

/// ailu-core#284: a request that asked for web search gets an outcome back — the scripted
/// one, or an empty one (no search ran, nothing was consulted). Deterministic, no network.
fn answer(request: &LlmRequest, mut response: LlmResponse) -> LlmResponse {
    if request.web_search.is_some() && response.web_search.is_none() {
        response.web_search = Some(WebSearchOutcome::default());
    }
    response
}

#[async_trait]
impl LlmProviderAdapter for MockAdapter {
    fn provider(&self) -> LlmProvider {
        self.provider
    }

    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        if self.responses.is_empty() {
            return Err(LlmError::Provider(
                "mock adapter has no responses".to_owned(),
            ));
        }
        let next = self.index.fetch_add(1, Ordering::SeqCst);
        let index = next.min(self.responses.len() - 1);
        Ok(answer(&request, self.responses[index].clone()))
    }

    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        if self.responses.is_empty() {
            return Err(LlmError::Provider(
                "mock adapter has no responses".to_owned(),
            ));
        }
        let next = self.index.fetch_add(1, Ordering::SeqCst);
        let index = next.min(self.responses.len() - 1);
        let response = answer(&request, self.responses[index].clone());
        match self.stream_scripts.get(index) {
            // Multi-chunk: replay the scripted deltas.
            Some(deltas) if !deltas.is_empty() => {
                for delta in deltas {
                    on_delta(delta);
                }
            }
            // Chunk-once: emit the whole content as one delta.
            _ => {
                if !response.content.is_empty() {
                    on_delta(&response.content);
                }
            }
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmMessage, LlmUsage, WebSearchConfig};

    fn request(web_search: Option<WebSearchConfig>) -> LlmRequest {
        LlmRequest {
            provider: LlmProvider::Mock,
            model: "mock".to_owned(),
            messages: vec![LlmMessage::text("user", "Hi")],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
            web_search,
        }
    }

    fn done() -> LlmResponse {
        LlmResponse {
            content: "done".to_owned(),
            tool_calls: None,
            stop_reason: None,
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Mock,
            content_blocks: None,
            web_search: None,
        }
    }

    #[tokio::test]
    async fn a_web_search_request_gets_an_empty_outcome_offline() {
        let adapter = MockAdapter::new(LlmProvider::Mock, vec![done()]);
        let config = WebSearchConfig {
            max_uses: 3,
            allowed_domains: None,
            blocked_domains: None,
        };
        let searched = adapter.complete(request(Some(config))).await.unwrap();
        assert_eq!(searched.web_search, Some(WebSearchOutcome::default()));
        let plain = adapter.complete(request(None)).await.unwrap();
        assert_eq!(plain.web_search, None);
    }
}
