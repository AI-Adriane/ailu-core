//! The gateway: route a request to the adapter registered for its provider. The
//! only seam onto real provider SDKs (a real adapter lands later behind this trait).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::LlmError;
use crate::media_resolver::{resolve_request_media, MediaResolver};
use crate::types::{LlmProvider, LlmRequest, LlmResponse};

/// Observational sink for per-token deltas during [`LlmProviderAdapter::stream`] /
/// [`LlmGateway::stream`] (ADR 0033, phase 13). The provider calls it once per delta as
/// generation streams in. It is **purely observational**: it must never influence the
/// returned [`LlmResponse`] — the return value is the authoritative, fully-assembled
/// response (byte-identical to [`LlmProviderAdapter::complete`]). Provider-agnostic by
/// design: the gateway layer knows nothing about `RunEvent`/`EventSink` (that lives above,
/// in `agents-core`/`bindings`); here a delta is just a `&str`.
pub type TokenSink<'a> = dyn Fn(&str) + Send + Sync + 'a;

#[async_trait]
pub trait LlmProviderAdapter: Send + Sync {
    fn provider(&self) -> LlmProvider;
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Stream the response, invoking `on_delta` once per token delta, and return the
    /// fully-assembled [`LlmResponse`] (identical in every field to [`Self::complete`]).
    ///
    /// Default impl is **chunk-once**: it calls `complete()` and emits the whole content
    /// as a single delta — so an adapter that has not yet implemented real SSE still
    /// streams correctly (just at one-chunk granularity). Real providers override this
    /// with provider SSE (Anthropic `messages.stream`, OpenAI `stream:true`, Gemini
    /// `:streamGenerateContent`).
    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let response = self.complete(request).await?;
        if !response.content.is_empty() {
            on_delta(&response.content);
        }
        Ok(response)
    }
}

#[async_trait]
pub trait LlmGateway: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Stream tokens for `request`, emitting each delta to `on_delta`, returning the
    /// assembled [`LlmResponse`]. Default chunk-once impl (calls `complete()`), so a
    /// non-streaming [`LlmGateway`] keeps working unchanged. [`DefaultLlmGateway`]
    /// overrides this to route to the adapter's real `stream()`.
    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let response = self.complete(request).await?;
        if !response.content.is_empty() {
            on_delta(&response.content);
        }
        Ok(response)
    }
}

#[derive(Default)]
pub struct DefaultLlmGateway {
    adapters: HashMap<LlmProvider, Box<dyn LlmProviderAdapter>>,
    /// ADR 0030 9c: resolves multimodal `Artifact` references to inline bytes at the boundary,
    /// before the adapter runs. `None` → no resolution (artifact refs reach adapters as-is and
    /// are skipped there; base64/url already work).
    media_resolver: Option<Arc<dyn MediaResolver>>,
}

impl DefaultLlmGateway {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Box<dyn LlmProviderAdapter>) {
        self.adapters.insert(adapter.provider(), adapter);
    }

    /// Install the media resolver (ADR 0030 9c) used to resolve `Artifact` media references.
    pub fn with_media_resolver(mut self, resolver: Arc<dyn MediaResolver>) -> Self {
        self.media_resolver = Some(resolver);
        self
    }
}

#[async_trait]
impl LlmGateway for DefaultLlmGateway {
    async fn complete(&self, mut request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // ADR 0030 9c: resolve any Artifact media references to inline bytes before dispatch,
        // so the adapter only ever sees base64/url (and oversized inline media is rejected).
        if let Some(resolver) = &self.media_resolver {
            resolve_request_media(&mut request, resolver.as_ref()).await?;
        }
        let adapter = self
            .adapters
            .get(&request.provider)
            .ok_or(LlmError::ProviderNotFound(request.provider))?;
        adapter.complete(request).await
    }

    async fn stream(
        &self,
        mut request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        // ADR 0030 9c: resolve Artifact media before dispatch, exactly like `complete`.
        if let Some(resolver) = &self.media_resolver {
            resolve_request_media(&mut request, resolver.as_ref()).await?;
        }
        let adapter = self
            .adapters
            .get(&request.provider)
            .ok_or(LlmError::ProviderNotFound(request.provider))?;
        adapter.stream(request, on_delta).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockAdapter;
    use crate::types::{LlmResponse, LlmUsage};

    fn response(content: &str) -> LlmResponse {
        LlmResponse {
            web_search: None,
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("end_turn".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    fn request() -> LlmRequest {
        LlmRequest {
            web_search: None,
            provider: LlmProvider::Anthropic,
            model: "claude".to_owned(),
            messages: vec![],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        }
    }

    #[tokio::test]
    async fn routes_to_the_registered_adapter() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![response("hello")],
        )));

        let result = gateway.complete(request()).await.unwrap();
        assert_eq!(result.content, "hello");
    }

    #[tokio::test]
    async fn errors_when_no_adapter_is_registered() {
        let gateway = DefaultLlmGateway::new();
        assert_eq!(
            gateway.complete(request()).await,
            Err(LlmError::ProviderNotFound(LlmProvider::Anthropic))
        );
    }

    /// ADR 0033 phase 13: `stream()` routes to the registered adapter, emits the scripted
    /// deltas in order, and returns the fully-assembled response (the authoritative one).
    #[tokio::test]
    async fn stream_routes_and_emits_scripted_deltas() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(
            MockAdapter::new(LlmProvider::Anthropic, vec![response("Hello world")])
                .with_stream_scripts(vec![vec!["Hello".to_owned(), " world".to_owned()]]),
        ));
        let collected = std::sync::Mutex::new(String::new());
        let result = gateway
            .stream(request(), &|delta: &str| {
                collected.lock().expect("lock").push_str(delta)
            })
            .await
            .unwrap();
        assert_eq!(*collected.lock().expect("lock"), "Hello world");
        // The return value is authoritative — identical to what `complete()` would yield.
        assert_eq!(result.content, "Hello world");
    }

    /// Default chunk-once: with no delta script, `stream()` emits the whole content once.
    #[tokio::test]
    async fn stream_chunk_once_emits_whole_content_as_one_delta() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![response("abc")],
        )));
        let chunks = std::sync::Mutex::new(Vec::<String>::new());
        let result = gateway
            .stream(request(), &|delta: &str| {
                chunks.lock().expect("lock").push(delta.to_owned())
            })
            .await
            .unwrap();
        assert_eq!(*chunks.lock().expect("lock"), vec!["abc".to_owned()]);
        assert_eq!(result.content, "abc");
    }

    #[tokio::test]
    async fn replays_scripted_responses_in_order() {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![response("first"), response("second")],
        )));

        assert_eq!(gateway.complete(request()).await.unwrap().content, "first");
        assert_eq!(gateway.complete(request()).await.unwrap().content, "second");
        // Exhausted → repeats the last.
        assert_eq!(gateway.complete(request()).await.unwrap().content, "second");
    }
}
