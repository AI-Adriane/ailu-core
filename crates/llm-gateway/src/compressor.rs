//! Prompt-compression seam (ADR 0014). The engine ships the trait, a no-op default, and a
//! generic HTTP client; the heavy compressor (LLMLingua-2, which drops low-information
//! tokens) runs as an external service behind `ADRIANE_LLMLINGUA_URL`. [`CompressingGateway`]
//! wraps any [`LlmGateway`] to shrink the **user-message** content (the bulky input/context)
//! before it reaches the provider — mirroring the [`crate::redactor`] seam.
//!
//! Compression is **lossy** (it removes words/punctuation), so it is opt-in via env and
//! only touches `user` messages — system instructions, assistant turns and tool results are
//! left intact. On any transport error it passes the text through (fail-open).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::gateway::LlmGateway;
use crate::types::{LlmRequest, LlmResponse};

/// Shrinks bulky prompt text before it reaches a provider.
#[async_trait]
pub trait PromptCompressor: Send + Sync {
    /// Return a possibly-shortened copy of `text`. Default seam impls fail-open (return
    /// the input unchanged on any error).
    async fn compress(&self, text: String) -> String;
}

/// Default: pass everything through unchanged (no compression).
pub struct NoopPromptCompressor;

#[async_trait]
impl PromptCompressor for NoopPromptCompressor {
    async fn compress(&self, text: String) -> String {
        text
    }
}

/// Wraps any [`LlmGateway`] so every `complete()` compresses the **user** message contents
/// first. Compose around the bare gateway: `CompressingGateway::new(inner, compressor)`.
pub struct CompressingGateway {
    inner: Arc<dyn LlmGateway>,
    compressor: Arc<dyn PromptCompressor>,
}

impl CompressingGateway {
    pub fn new(inner: Arc<dyn LlmGateway>, compressor: Arc<dyn PromptCompressor>) -> Self {
        Self { inner, compressor }
    }
}

#[async_trait]
impl LlmGateway for CompressingGateway {
    async fn complete(&self, mut request: LlmRequest) -> Result<LlmResponse, LlmError> {
        for message in request.messages.iter_mut() {
            if message.role == "user" {
                let text = std::mem::take(&mut message.content);
                message.content = self.compressor.compress(text).await;
            }
        }
        self.inner.complete(request).await
    }
}

#[derive(Serialize)]
struct CompressRequest {
    text: String,
    rate: f64,
}

#[derive(Deserialize)]
struct CompressResponse {
    compressed: String,
}

/// Calls an external LLMLingua compression service over HTTP. Configure with
/// `ADRIANE_LLMLINGUA_URL` (the service's `POST { text, rate } -> { compressed }` endpoint),
/// `ADRIANE_LLMLINGUA_RATE` (target keep-ratio, default 0.5) and `ADRIANE_LLMLINGUA_MIN_CHARS`
/// (skip texts shorter than this, default 240). Returns `None` from `from_env` when the URL
/// is unset, so the caller skips wrapping and the engine runs with no compression.
pub struct HttpPromptCompressor {
    url: String,
    rate: f64,
    min_chars: usize,
    client: reqwest::Client,
}

impl HttpPromptCompressor {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("ADRIANE_LLMLINGUA_URL")
            .ok()
            .filter(|value| !value.is_empty())?;
        let rate = std::env::var("ADRIANE_LLMLINGUA_RATE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.5);
        let min_chars = std::env::var("ADRIANE_LLMLINGUA_MIN_CHARS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(240);
        Some(Self {
            url,
            rate,
            min_chars,
            client: crate::http::http_client(),
        })
    }
}

#[async_trait]
impl PromptCompressor for HttpPromptCompressor {
    async fn compress(&self, text: String) -> String {
        if text.chars().count() < self.min_chars {
            return text;
        }
        let request = CompressRequest {
            text: text.clone(),
            rate: self.rate,
        };
        match self.client.post(&self.url).json(&request).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.json::<CompressResponse>().await {
                    Ok(body) => body.compressed,
                    Err(_) => text,
                },
                Err(_) => text,
            },
            Err(error) => {
                eprintln!("[llmlingua] compression service error, passing text through: {error}");
                text
            }
        }
    }
}
