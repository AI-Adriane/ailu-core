//! OpenAI-compatible (Chat Completions) provider adapter — the Rust port of
//! `@adriane-ai/llm-gateway`'s `openai-compatible-adapter.ts`, mirroring the
//! [`crate::anthropic`] adapter's port-seam pattern.
//!
//! One adapter drives any server speaking the OpenAI `/chat/completions` shape:
//! both a local **Ollama** server (`http://localhost:11434/v1`, keyless) and
//! **Mistral cloud** (`https://api.mistral.ai/v1`, bearer key) go through this
//! same struct. Construct them with [`OpenAiCompatibleAdapter::ollama`] /
//! [`OpenAiCompatibleAdapter::mistral`].
//!
//! Request assembly mirrors the TS adapter:
//! - `req.system` folds in as the **first** `{role:"system", content}` message,
//! - string-content messages pass through verbatim (the Rust [`LlmMessage`] is
//!   text-only, so there is no content-block fan-out yet),
//! - tools map to `[{type:"function", function:{name, description, parameters}}]`
//!   **only when non-empty** — providers 400 on an empty `tools` array,
//! - `temperature` / `max_tokens` are emitted only when present,
//! - a model that doesn't look like a provider model id (Anthropic ids, agent
//!   placeholders) resolves to the default model.
//!
//! The transport is a single seam, [`OpenAiCompatiblePort`], so the request /
//! response mapping is fully covered without a network call or an API key.
//! [`HttpPort`] is the real reqwest-backed implementation behind the same trait;
//! the pure [`build_request_body`] carries the wire-shape logic and is unit-tested.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::LlmError;
use crate::gateway::{LlmProviderAdapter, TokenSink};
use crate::sse::SseDecoder;
use crate::types::{
    ContentBlock, LlmProvider, LlmRequest, LlmResponse, LlmToolCall, LlmUsage, ResponseFormat,
};

/// Mistral cloud base URL.
pub const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
/// Default Mistral model when the request does not name one.
pub const MISTRAL_DEFAULT_MODEL: &str = "mistral-small-latest";
/// Local Ollama OpenAI-compatible base URL.
pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
/// Default Ollama model when the request does not name one.
pub const OLLAMA_DEFAULT_MODEL: &str = "mistral";
/// OpenAI cloud base URL.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Default OpenAI model when the request does not name one.
pub const OPENAI_DEFAULT_MODEL: &str = "gpt-4o-mini";
/// OpenRouter base URL (aggregates many providers behind one OpenAI-shaped API).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Default OpenRouter model when the request does not name one (ids are namespaced).
pub const OPENROUTER_DEFAULT_MODEL: &str = "openai/gpt-4o-mini";
/// MiniMax cloud base URL.
pub const MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
/// Default MiniMax model when the request does not name one.
pub const MINIMAX_DEFAULT_MODEL: &str = "MiniMax-Text-01";
/// Hugging Face inference router base URL (OpenAI-compatible).
pub const HUGGINGFACE_BASE_URL: &str = "https://router.huggingface.co/v1";
/// Default Hugging Face model when the request does not name one.
pub const HUGGINGFACE_DEFAULT_MODEL: &str = "meta-llama/Llama-3.3-70B-Instruct";
/// Local LM Studio OpenAI-compatible base URL.
pub const LMSTUDIO_BASE_URL: &str = "http://localhost:1234/v1";
/// Default LM Studio model id — LM Studio serves whatever model is loaded, so the
/// request usually pins the id and this is only a keyless fallback label.
pub const LMSTUDIO_DEFAULT_MODEL: &str = "local-model";

// ---------------------------------------------------------------------------
// Raw wire response (snake_case, matches the OpenAI chat-completions JSON)
// ---------------------------------------------------------------------------

/// One tool call inside a choice's message. `arguments` arrives as a JSON
/// **string** that the adapter parses into the structured input.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RawFunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RawToolCall {
    pub id: String,
    #[serde(default, rename = "type")]
    pub call_type: Option<String>,
    pub function: RawFunctionCall,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<RawToolCall>>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawChoice {
    pub message: RawMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RawUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    /// Cached-prompt accounting (OpenAI-style + Gemini's OpenAI-compat endpoint return
    /// `prompt_tokens_details.cached_tokens` when a prefix was served from cache).
    #[serde(default)]
    pub prompt_tokens_details: Option<RawPromptTokensDetails>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RawPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

/// Structural subset of the OpenAI chat-completion response the adapter reads.
/// Deserializes straight from the provider's wire JSON (snake_case).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct OpenAiChatResponse {
    #[serde(default)]
    pub choices: Vec<RawChoice>,
    #[serde(default)]
    pub usage: Option<RawUsage>,
}

/// Reassembles a streaming chat-completion from its SSE chunks (ADR 0033, phase 13).
/// Pure + offline-testable: feed each chunk's `data:` JSON to [`Self::push_event`] (which
/// returns a text delta when the chunk carried `delta.content`), then [`Self::finish`]
/// yields a [`OpenAiChatResponse`]-shaped `Value` the adapter maps via `to_response` — so
/// the assembled response is identical to the non-streaming path.
#[derive(Default)]
pub struct OpenAiStreamAccumulator {
    content: String,
    tool_calls: Vec<StreamingToolCall>,
    finish_reason: Option<String>,
    usage: Option<Value>,
}

#[derive(Default)]
struct StreamingToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl OpenAiStreamAccumulator {
    fn ensure_tool(&mut self, index: usize) {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(StreamingToolCall::default());
        }
    }

    /// Process one SSE `data:` payload (`[DONE]` is a no-op). Returns the text delta when
    /// the chunk carried `choices[0].delta.content`.
    pub fn push_event(&mut self, data: &str) -> Option<String> {
        if data.trim() == "[DONE]" {
            return None;
        }
        let value: Value = serde_json::from_str(data).ok()?;
        // Usage rides a trailing chunk (with `stream_options.include_usage`) whose
        // `choices` is empty — read it before bailing on the missing choice.
        if let Some(usage) = value.get("usage") {
            if !usage.is_null() {
                self.usage = Some(usage.clone());
            }
        }
        let choice = value.get("choices")?.get(0)?;
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
        let delta = choice.get("delta")?;
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.ensure_tool(index);
                let entry = &mut self.tool_calls[index];
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        entry.id = id.to_owned();
                    }
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    if !name.is_empty() {
                        entry.name = name.to_owned();
                    }
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(args);
                }
            }
        }
        let text = delta.get("content").and_then(Value::as_str)?;
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_owned())
    }

    /// Emit the assembled response as a chat-completion `Value`.
    pub fn finish(self) -> Value {
        let tool_calls: Vec<Value> = self
            .tool_calls
            .into_iter()
            .filter(|call| !call.id.is_empty() || !call.name.is_empty())
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments }
                })
            })
            .collect();
        let mut message = json!({ "content": self.content });
        if !tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(tool_calls);
        }
        let mut response = json!({
            "choices": [{ "message": message, "finish_reason": self.finish_reason }]
        });
        if let Some(usage) = self.usage {
            response["usage"] = usage;
        }
        response
    }
}

// ---------------------------------------------------------------------------
// Port seam
// ---------------------------------------------------------------------------

/// The only seam onto the HTTP transport. Production uses [`HttpPort`]; tests
/// supply a fake so the request / response mapping is covered without a network
/// call. The body is the already-assembled OpenAI chat-completions JSON.
#[async_trait]
pub trait OpenAiCompatiblePort: Send + Sync {
    async fn send(&self, body: Value) -> Result<Value, LlmError>;

    /// Stream the chat-completion, calling `on_delta` per text delta, returning the
    /// fully-assembled response `Value` (ADR 0033, phase 13). Default impl is chunk-once:
    /// it `send()`s and emits the assembled content as one delta — so a test port streams
    /// without real SSE. Only [`HttpPort`] overrides this with the provider's event stream.
    async fn send_stream(&self, body: Value, on_delta: &TokenSink<'_>) -> Result<Value, LlmError> {
        let raw = self.send(body).await?;
        let text = raw
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !text.is_empty() {
            on_delta(text);
        }
        Ok(raw)
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// One adapter for any OpenAI `/chat/completions`-compatible server. The gateway
/// slot it registers under is carried by [`OpenAiCompatibleAdapter::provider`], so
/// several OpenAI-compatible providers (OpenAI, Mistral, OpenRouter, MiniMax,
/// Hugging Face, LM Studio, Ollama) can coexist in one gateway — each in its own slot.
pub struct OpenAiCompatibleAdapter {
    port: Box<dyn OpenAiCompatiblePort>,
    provider: LlmProvider,
    default_model: String,
}

impl OpenAiCompatibleAdapter {
    /// Build an adapter over the given port with an explicit default model,
    /// registering under the [`LlmProvider::Mistral`] slot. Kept for the existing
    /// call sites and tests; new providers use [`OpenAiCompatibleAdapter::with_provider`]
    /// or a named constructor.
    pub fn new(port: Box<dyn OpenAiCompatiblePort>, default_model: impl Into<String>) -> Self {
        Self::with_provider(port, LlmProvider::Mistral, default_model)
    }

    /// Build an adapter over the given port, naming the gateway slot it registers
    /// under. The request's `provider` must match this slot for the gateway to route
    /// to it.
    pub fn with_provider(
        port: Box<dyn OpenAiCompatiblePort>,
        provider: LlmProvider,
        default_model: impl Into<String>,
    ) -> Self {
        OpenAiCompatibleAdapter {
            port,
            provider,
            default_model: default_model.into(),
        }
    }

    /// Mistral cloud: bearer-keyed, hosted at [`MISTRAL_BASE_URL`]. `model`
    /// overrides the [`MISTRAL_DEFAULT_MODEL`] fallback. Pass `api_key: None`
    /// only for testing against a keyless stub.
    pub fn mistral(api_key: Option<String>, model: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| MISTRAL_DEFAULT_MODEL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(MISTRAL_BASE_URL, api_key)),
            provider: LlmProvider::Mistral,
            default_model,
        }
    }

    /// A local Ollama server (keyless, [`OLLAMA_BASE_URL`] by default), registered
    /// under the [`LlmProvider::Ollama`] slot. `model` overrides
    /// [`OLLAMA_DEFAULT_MODEL`]; `base_url` overrides the host.
    pub fn ollama(model: Option<String>, base_url: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| OLLAMA_DEFAULT_MODEL.to_owned());
        let base = base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(base, None)),
            provider: LlmProvider::Ollama,
            default_model,
        }
    }

    /// OpenAI cloud: bearer-keyed, hosted at [`OPENAI_BASE_URL`]. `model` overrides
    /// the [`OPENAI_DEFAULT_MODEL`] fallback.
    pub fn openai(api_key: Option<String>, model: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| OPENAI_DEFAULT_MODEL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(OPENAI_BASE_URL, api_key)),
            provider: LlmProvider::Openai,
            default_model,
        }
    }

    /// A caller-supplied OpenAI-compatible endpoint (vLLM, LM Studio, a gateway, …) registered
    /// under `provider`'s slot. `api_key` is sent as the bearer token when present (keyless
    /// servers pass `None`); `model` is the default when a request names none.
    pub fn custom_endpoint(
        base_url: impl Into<String>,
        provider: LlmProvider,
        api_key: Option<String>,
        model: Option<String>,
    ) -> Self {
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(base_url, api_key)),
            provider,
            default_model: model.unwrap_or_default(),
        }
    }

    /// OpenRouter: bearer-keyed, hosted at [`OPENROUTER_BASE_URL`]. Model ids are
    /// namespaced (e.g. `openai/gpt-4o`); `model` overrides [`OPENROUTER_DEFAULT_MODEL`].
    pub fn openrouter(api_key: Option<String>, model: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| OPENROUTER_DEFAULT_MODEL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(OPENROUTER_BASE_URL, api_key)),
            provider: LlmProvider::Openrouter,
            default_model,
        }
    }

    /// MiniMax cloud: bearer-keyed, hosted at [`MINIMAX_BASE_URL`]. `model` overrides
    /// the [`MINIMAX_DEFAULT_MODEL`] fallback.
    pub fn minimax(api_key: Option<String>, model: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| MINIMAX_DEFAULT_MODEL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(MINIMAX_BASE_URL, api_key)),
            provider: LlmProvider::Minimax,
            default_model,
        }
    }

    /// Hugging Face inference router: bearer-keyed, hosted at [`HUGGINGFACE_BASE_URL`].
    /// `model` overrides the [`HUGGINGFACE_DEFAULT_MODEL`] fallback.
    pub fn huggingface(api_key: Option<String>, model: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| HUGGINGFACE_DEFAULT_MODEL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(HUGGINGFACE_BASE_URL, api_key)),
            provider: LlmProvider::Huggingface,
            default_model,
        }
    }

    /// A local LM Studio server (keyless, [`LMSTUDIO_BASE_URL`] by default),
    /// registered under the [`LlmProvider::Lmstudio`] slot. `model` overrides
    /// [`LMSTUDIO_DEFAULT_MODEL`]; `base_url` overrides the host.
    pub fn lmstudio(model: Option<String>, base_url: Option<String>) -> Self {
        let default_model = model.unwrap_or_else(|| LMSTUDIO_DEFAULT_MODEL.to_owned());
        let base = base_url.unwrap_or_else(|| LMSTUDIO_BASE_URL.to_owned());
        OpenAiCompatibleAdapter {
            port: Box::new(HttpPort::new(base, None)),
            provider: LlmProvider::Lmstudio,
            default_model,
        }
    }

    fn to_response(&self, request: &LlmRequest, model: String, raw: Value) -> LlmResponse {
        // Deserialize defensively: an unexpected shape yields empty defaults
        // rather than an error, mirroring the TS adapter's optional chaining.
        let parsed: OpenAiChatResponse = serde_json::from_value(raw).unwrap_or_default();
        to_response(request, model, parsed)
    }
}

fn to_response(request: &LlmRequest, model: String, raw: OpenAiChatResponse) -> LlmResponse {
    let choice = raw.choices.into_iter().next().unwrap_or_default();
    let content = choice.message.content.unwrap_or_default();

    let tool_calls: Vec<LlmToolCall> = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| LlmToolCall {
            id: call.id,
            name: call.function.name,
            input: parse_arguments(&call.function.arguments),
        })
        .collect();

    let usage = raw.usage.unwrap_or_default();

    LlmResponse {
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        stop_reason: choice.finish_reason,
        usage: LlmUsage {
            prompt_tokens: usage.prompt_tokens.unwrap_or(0),
            completion_tokens: usage.completion_tokens.unwrap_or(0),
            cache_read_tokens: usage
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens),
            cache_write_tokens: None,
        },
        model,
        provider: request.provider,
        content_blocks: None,
    }
}

#[async_trait]
impl LlmProviderAdapter for OpenAiCompatibleAdapter {
    fn provider(&self) -> LlmProvider {
        self.provider
    }

    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = build_request_body(&request, &self.default_model);
        // The resolved model is `body["model"]` — read it back for the response.
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_model)
            .to_owned();
        let raw = self.port.send(body).await?;
        Ok(self.to_response(&request, model, raw))
    }

    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let mut body = build_request_body(&request, &self.default_model);
        // Ask the server to stream and to include usage on the final chunk.
        if let Value::Object(map) = &mut body {
            map.insert("stream".to_owned(), json!(true));
            map.insert(
                "stream_options".to_owned(),
                json!({ "include_usage": true }),
            );
        }
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&self.default_model)
            .to_owned();
        // The assembled response is authoritative — same `to_response` mapping as `complete()`.
        let raw = self.port.send_stream(body, on_delta).await?;
        Ok(self.to_response(&request, model, raw))
    }
}

// ---------------------------------------------------------------------------
// Pure request mapping (the seam tests assert against directly)
// ---------------------------------------------------------------------------

/// Map an [`LlmRequest`] to the OpenAI chat-completions body. Pure, so tests
/// assert on it directly. `req.system` folds in as the first `system` message;
/// string-content messages pass through with their role. Tools are emitted
/// **only when non-empty** — never an empty `tools` array. `temperature` /
/// `max_tokens` appear only when present.
pub fn build_request_body(req: &LlmRequest, default_model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(system) = &req.system {
        if !system.is_empty() {
            messages.push(json!({ "role": "system", "content": system }));
        }
    }

    fn openai_content_parts(blocks: &[ContentBlock]) -> Vec<Value> {
        // Each block → an OpenAI content part. Image/file via a (data-URI or URL) `url`;
        // audio via inline base64 `input_audio`. An unresolved Artifact source yields no
        // url/data and is skipped (the gateway resolves artifacts before the adapter runs).
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(json!({ "type": "text", "text": text })),
                ContentBlock::Image { source } => source
                    .as_url_or_data_uri()
                    .map(|url| json!({ "type": "image_url", "image_url": { "url": url } })),
                ContentBlock::Audio { source } => source.as_base64().map(|(media_type, data)| {
                    let format = media_type.rsplit('/').next().unwrap_or("mp3");
                    json!({ "type": "input_audio", "input_audio": { "data": data, "format": format } })
                }),
                ContentBlock::File { source } => source
                    .as_url_or_data_uri()
                    .map(|url| json!({ "type": "file", "file": { "file_data": url } })),
            })
            .collect()
    }

    for message in &req.messages {
        let mut m = Map::new();
        m.insert("role".to_owned(), json!(message.role));
        // ADR 0030: multimodal — when content blocks are present, content becomes an array of
        // typed parts; otherwise it stays a plain string (byte-identical to before).
        match &message.content_blocks {
            Some(blocks) => {
                m.insert(
                    "content".to_owned(),
                    Value::Array(openai_content_parts(blocks)),
                );
            }
            None => {
                m.insert("content".to_owned(), json!(message.content));
            }
        }
        // Assistant tool calls → OpenAI `tool_calls` (function `arguments` is a JSON string).
        if let Some(calls) = &message.tool_calls {
            m.insert(
                "tool_calls".to_owned(),
                Value::Array(
                    calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": { "name": c.name, "arguments": c.input.to_string() }
                            })
                        })
                        .collect(),
                ),
            );
        }
        // Tool-result message → link it back to the assistant's call id.
        if let Some(id) = &message.tool_call_id {
            m.insert("tool_call_id".to_owned(), json!(id));
        }
        messages.push(Value::Object(m));
    }

    let mut body = Map::new();
    body.insert(
        "model".to_owned(),
        json!(resolve_model(&req.model, default_model)),
    );
    body.insert("messages".to_owned(), Value::Array(messages));

    // Never emit an empty tools array: providers 400 on it.
    if let Some(tools) = &req.tools {
        if !tools.is_empty() {
            body.insert(
                "tools".to_owned(),
                Value::Array(
                    tools
                        .iter()
                        .map(|tool| {
                            let mut function = Map::new();
                            function.insert("name".to_owned(), json!(tool.name));
                            if let Some(description) = &tool.description {
                                function.insert("description".to_owned(), json!(description));
                            }
                            function.insert("parameters".to_owned(), tool.input_schema.clone());
                            json!({ "type": "function", "function": Value::Object(function) })
                        })
                        .collect(),
                ),
            );
        }
    }

    if let Some(temperature) = req.temperature {
        body.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = req.max_tokens {
        body.insert("max_tokens".to_owned(), json!(max_tokens));
    }

    // ADR 0029: native JSON-schema-constrained generation. OpenAI-compatible providers take
    // a top-level `response_format`. Aggregators / local servers that don't support it ignore
    // the field; the in-engine validation floor still enforces conformance.
    if let Some(ResponseFormat::JsonSchema {
        name,
        schema,
        strict,
    }) = &req.response_format
    {
        body.insert(
            "response_format".to_owned(),
            json!({
                "type": "json_schema",
                "json_schema": { "name": name, "schema": schema, "strict": strict }
            }),
        );
    }

    Value::Object(body)
}

/// Keep an explicit model id; otherwise fall back to the provider default.
fn resolve_model(model: &str, default_model: &str) -> String {
    if looks_like_model_id(model) {
        model.to_owned()
    } else {
        default_model.to_owned()
    }
}

/// Heuristic for "is this a real model id for this provider" vs an agent
/// placeholder (e.g. `claude-opus-4-8`, `react-agent`). Anthropic ids and the
/// agent's default Claude model don't belong here, so route them onto the
/// provider default instead.
fn looks_like_model_id(model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    // Namespaced ids (OpenRouter, Hugging Face) like `openai/gpt-4o` or
    // `anthropic/claude-3.5-sonnet` are always explicit provider model ids.
    if model.contains('/') {
        return true;
    }
    if model.starts_with("claude-") {
        return false;
    }
    if model == "react-agent" || model == "mock" || model == "mock-model" {
        return false;
    }
    true
}

/// Tool-call arguments arrive as a JSON string; parse defensively, default to `{}`.
fn parse_arguments(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(Map::new()))
}

// ---------------------------------------------------------------------------
// HTTP port (the only code that touches the network)
// ---------------------------------------------------------------------------

/// Real [`OpenAiCompatiblePort`] over `POST {base_url}/chat/completions`. Sends
/// `Authorization: Bearer <key>` when a key is present + `content-type:
/// application/json`. A non-2xx response surfaces as [`LlmError::Provider`] with
/// the status and body. Never exercised in tests.
pub struct HttpPort {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl HttpPort {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        HttpPort {
            client: crate::http::http_client(),
            base_url: base_url.into(),
            api_key,
        }
    }
}

#[async_trait]
impl OpenAiCompatiblePort for HttpPort {
    async fn send(&self, body: Value) -> Result<Value, LlmError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut builder = self
            .client
            .post(url)
            .header("content-type", "application/json");
        if let Some(api_key) = &self.api_key {
            if !api_key.is_empty() {
                builder = builder.header("authorization", format!("Bearer {api_key}"));
            }
        }

        let response = builder.json(&body).send().await.map_err(|err| {
            LlmError::Provider(format!("openai-compatible request failed: {err}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!(
                "openai-compatible returned {status}: {text}"
            )));
        }

        response.json::<Value>().await.map_err(|err| {
            LlmError::Provider(format!("openai-compatible response decode failed: {err}"))
        })
    }

    /// Real SSE over `POST {base_url}/chat/completions` with `"stream": true`. Frames the
    /// byte stream and folds the chunks through [`OpenAiStreamAccumulator`], emitting each
    /// text delta live. Transport verified on a live key (probe), not in CI — like `send()`;
    /// the chunk reassembly is unit-tested offline through the accumulator.
    async fn send_stream(&self, body: Value, on_delta: &TokenSink<'_>) -> Result<Value, LlmError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut builder = self
            .client
            .post(url)
            .header("content-type", "application/json");
        if let Some(api_key) = &self.api_key {
            if !api_key.is_empty() {
                builder = builder.header("authorization", format!("Bearer {api_key}"));
            }
        }

        let response = builder.json(&body).send().await.map_err(|err| {
            LlmError::Provider(format!("openai-compatible stream request failed: {err}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!(
                "openai-compatible returned {status}: {text}"
            )));
        }

        let mut decoder = SseDecoder::default();
        let mut accumulator = OpenAiStreamAccumulator::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|err| {
                LlmError::Provider(format!("openai-compatible stream read failed: {err}"))
            })?;
            let text = String::from_utf8_lossy(&bytes);
            for payload in decoder.push(&text) {
                if let Some(delta) = accumulator.push_event(&payload) {
                    on_delta(&delta);
                }
            }
        }
        if let Some(payload) = decoder.finish() {
            if let Some(delta) = accumulator.push_event(&payload) {
                on_delta(&delta);
            }
        }
        Ok(accumulator.finish())
    }
}

// ---------------------------------------------------------------------------
// Tests (mirror the TS suite — no network, fake recording port)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::gateway::{DefaultLlmGateway, LlmGateway};
    use crate::types::{LlmMessage, LlmToolDef, MediaSource, ResponseFormat};

    /// Captures each body the adapter sends and returns a canned response.
    struct RecordingPort {
        bodies: Arc<Mutex<Vec<Value>>>,
        response: Value,
    }

    #[async_trait]
    impl OpenAiCompatiblePort for RecordingPort {
        async fn send(&self, body: Value) -> Result<Value, LlmError> {
            self.bodies.lock().unwrap().push(body);
            Ok(self.response.clone())
        }
    }

    fn recording_port(response: Value) -> (Box<dyn OpenAiCompatiblePort>, Arc<Mutex<Vec<Value>>>) {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let port = RecordingPort {
            bodies: Arc::clone(&bodies),
            response,
        };
        (Box::new(port), bodies)
    }

    /// A port that always errors — to cover the non-2xx surfacing through `complete`.
    struct FailingPort;

    #[async_trait]
    impl OpenAiCompatiblePort for FailingPort {
        async fn send(&self, _body: Value) -> Result<Value, LlmError> {
            Err(LlmError::Provider(
                "openai-compatible returned 400 Bad Request: bad".to_owned(),
            ))
        }
    }

    fn text_response() -> Value {
        json!({
            "choices": [
                { "message": { "content": "hello" }, "finish_reason": "stop" }
            ],
            "usage": { "prompt_tokens": 12, "completion_tokens": 8 }
        })
    }

    fn base_request() -> LlmRequest {
        LlmRequest {
            provider: LlmProvider::Mistral,
            model: "mistral-small-latest".to_owned(),
            messages: vec![LlmMessage::text("user", "Hi")],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            run_id: None,
            response_format: None,
        }
    }

    #[tokio::test]
    async fn folds_system_first_and_maps_tools_with_input_schema_as_parameters() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let request = LlmRequest {
            system: Some("You are a helpful agent.".to_owned()),
            tools: Some(vec![LlmToolDef {
                name: "search".to_owned(),
                description: Some("Search things".to_owned()),
                input_schema: json!({ "type": "object", "properties": { "query": { "type": "string" } } }),
            }]),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let bodies = bodies.lock().unwrap();
        let body = &bodies[0];

        // req.system folds in as the FIRST message.
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[0]["content"], json!("You are a helpful agent."));
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"], json!("Hi"));

        // Tools map to function shape; parameters == input_schema verbatim.
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[0]["type"], json!("function"));
        assert_eq!(tools[0]["function"]["name"], json!("search"));
        assert_eq!(tools[0]["function"]["description"], json!("Search things"));
        assert_eq!(
            tools[0]["function"]["parameters"],
            json!({ "type": "object", "properties": { "query": { "type": "string" } } })
        );
    }

    #[tokio::test]
    async fn response_format_maps_to_top_level_json_schema() {
        // ADR 0029: an OpenAI-compatible request carries a top-level `response_format`.
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let request = LlmRequest {
            response_format: Some(ResponseFormat::JsonSchema {
                name: "Verdict".to_owned(),
                schema: json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } }),
                strict: true,
            }),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let bodies = bodies.lock().unwrap();
        let rf = &bodies[0]["response_format"];
        assert_eq!(rf["type"], json!("json_schema"));
        assert_eq!(rf["json_schema"]["name"], json!("Verdict"));
        assert_eq!(rf["json_schema"]["strict"], json!(true));
        assert_eq!(
            rf["json_schema"]["schema"],
            json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } })
        );
    }

    #[tokio::test]
    async fn image_content_block_maps_to_an_image_url_part() {
        // ADR 0030: a message with content blocks → an OpenAI content ARRAY of typed parts.
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let request = LlmRequest {
            messages: vec![LlmMessage::with_blocks(
                "user",
                vec![
                    ContentBlock::Text {
                        text: "what is this?".to_owned(),
                    },
                    ContentBlock::Image {
                        source: MediaSource::Base64 {
                            media_type: "image/png".to_owned(),
                            data: "AAAA".to_owned(),
                        },
                    },
                ],
            )],
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let bodies = bodies.lock().unwrap();
        let msgs = bodies[0]["messages"].as_array().unwrap();
        let user = msgs.iter().find(|m| m["content"].is_array()).unwrap();
        let parts = user["content"].as_array().unwrap();
        assert_eq!(parts[0], json!({ "type": "text", "text": "what is this?" }));
        assert_eq!(parts[1]["type"], json!("image_url"));
        assert_eq!(
            parts[1]["image_url"]["url"],
            json!("data:image/png;base64,AAAA")
        );
    }

    #[tokio::test]
    async fn url_image_maps_to_the_url_verbatim() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);
        let request = LlmRequest {
            messages: vec![LlmMessage::with_blocks(
                "user",
                vec![ContentBlock::Image {
                    source: MediaSource::Url {
                        url: "https://cdn/x.png".to_owned(),
                        media_type: None,
                    },
                }],
            )],
            ..base_request()
        };
        adapter.complete(request).await.unwrap();
        let bodies = bodies.lock().unwrap();
        let msgs = bodies[0]["messages"].as_array().unwrap();
        let user = msgs.iter().find(|m| m["content"].is_array()).unwrap();
        assert_eq!(
            user["content"][0]["image_url"]["url"],
            json!("https://cdn/x.png")
        );
    }

    #[tokio::test]
    async fn never_emits_a_tools_key_when_there_are_no_tools() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        // No tools at all, and an explicitly empty list — both must omit `tools`.
        adapter.complete(base_request()).await.unwrap();
        let request = LlmRequest {
            tools: Some(vec![]),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let bodies = bodies.lock().unwrap();
        assert!(bodies[0].get("tools").is_none());
        assert!(bodies[1].get("tools").is_none());
    }

    #[tokio::test]
    async fn maps_text_only_completion_and_usage() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let request = LlmRequest {
            temperature: Some(0.5),
            response_format: None,
            max_tokens: Some(256),
            ..base_request()
        };
        let result = adapter.complete(request).await.unwrap();

        assert_eq!(result.content, "hello");
        assert_eq!(result.tool_calls, None);
        assert_eq!(result.stop_reason.as_deref(), Some("stop"));
        assert_eq!(result.usage.prompt_tokens, 12);
        assert_eq!(result.usage.completion_tokens, 8);
        assert_eq!(result.provider, LlmProvider::Mistral);

        // temperature / max_tokens are emitted only when present.
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies[0]["temperature"], json!(0.5));
        assert_eq!(bodies[0]["max_tokens"], json!(256));
    }

    #[tokio::test]
    async fn omits_temperature_and_max_tokens_when_absent() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        adapter.complete(base_request()).await.unwrap();

        let bodies = bodies.lock().unwrap();
        assert!(bodies[0].get("temperature").is_none());
        assert!(bodies[0].get("max_tokens").is_none());
    }

    #[tokio::test]
    async fn parses_tool_calls_with_json_string_arguments() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "search", "arguments": "{\"query\":\"adriane\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let (port, _bodies) = recording_port(response);
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let result = adapter.complete(base_request()).await.unwrap();

        assert_eq!(result.content, "");
        assert_eq!(result.stop_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            result.tool_calls,
            Some(vec![LlmToolCall {
                id: "call_1".to_owned(),
                name: "search".to_owned(),
                input: json!({ "query": "adriane" }),
            }])
        );
    }

    #[tokio::test]
    async fn surfaces_a_non_2xx_as_a_provider_error() {
        let adapter = OpenAiCompatibleAdapter::new(Box::new(FailingPort), MISTRAL_DEFAULT_MODEL);

        let error = adapter.complete(base_request()).await.unwrap_err();

        match error {
            LlmError::Provider(message) => {
                assert!(message.contains("400"));
                assert!(message.contains("bad"));
            }
            other => panic!("expected a provider error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn routes_through_the_default_gateway_on_mistral() {
        let (port, _bodies) = recording_port(text_response());
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::new(
            port,
            MISTRAL_DEFAULT_MODEL,
        )));

        let result = gateway.complete(base_request()).await.unwrap();

        assert_eq!(result.content, "hello");
        assert_eq!(result.provider, LlmProvider::Mistral);
    }

    #[tokio::test]
    async fn falls_back_to_the_default_model_for_placeholder_ids() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        // `claude-opus-4-8` is the agent placeholder, not a Mistral model id.
        let request = LlmRequest {
            model: "claude-opus-4-8".to_owned(),
            ..base_request()
        };
        let result = adapter.complete(request).await.unwrap();

        assert_eq!(
            bodies.lock().unwrap()[0]["model"],
            json!("mistral-small-latest")
        );
        assert_eq!(result.model, "mistral-small-latest");
    }

    #[tokio::test]
    async fn keeps_an_explicit_provider_model_id() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::new(port, MISTRAL_DEFAULT_MODEL);

        let request = LlmRequest {
            model: "mistral-large-latest".to_owned(),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        assert_eq!(
            bodies.lock().unwrap()[0]["model"],
            json!("mistral-large-latest")
        );
    }

    #[test]
    fn mistral_constructor_uses_the_hosted_defaults() {
        let adapter = OpenAiCompatibleAdapter::mistral(Some("sk-test".to_owned()), None);
        assert_eq!(adapter.provider(), LlmProvider::Mistral);
        assert_eq!(adapter.default_model, MISTRAL_DEFAULT_MODEL);

        let custom =
            OpenAiCompatibleAdapter::mistral(None, Some("mistral-large-latest".to_owned()));
        assert_eq!(custom.default_model, "mistral-large-latest");
    }

    #[test]
    fn ollama_constructor_uses_the_local_defaults() {
        let adapter = OpenAiCompatibleAdapter::ollama(None, None);
        assert_eq!(adapter.provider(), LlmProvider::Ollama);
        assert_eq!(adapter.default_model, OLLAMA_DEFAULT_MODEL);

        let custom = OpenAiCompatibleAdapter::ollama(
            Some("llama3".to_owned()),
            Some("http://example.test/v1".to_owned()),
        );
        assert_eq!(custom.default_model, "llama3");
    }

    #[test]
    fn named_constructors_register_under_their_own_provider_slot() {
        // Each hosted provider lands in its own gateway slot with its own default model.
        let openai = OpenAiCompatibleAdapter::openai(Some("sk".to_owned()), None);
        assert_eq!(openai.provider(), LlmProvider::Openai);
        assert_eq!(openai.default_model, OPENAI_DEFAULT_MODEL);

        let openrouter = OpenAiCompatibleAdapter::openrouter(Some("sk".to_owned()), None);
        assert_eq!(openrouter.provider(), LlmProvider::Openrouter);
        assert_eq!(openrouter.default_model, OPENROUTER_DEFAULT_MODEL);

        let minimax = OpenAiCompatibleAdapter::minimax(Some("sk".to_owned()), None);
        assert_eq!(minimax.provider(), LlmProvider::Minimax);
        assert_eq!(minimax.default_model, MINIMAX_DEFAULT_MODEL);

        let hf = OpenAiCompatibleAdapter::huggingface(Some("hf".to_owned()), None);
        assert_eq!(hf.provider(), LlmProvider::Huggingface);
        assert_eq!(hf.default_model, HUGGINGFACE_DEFAULT_MODEL);

        let lmstudio = OpenAiCompatibleAdapter::lmstudio(None, None);
        assert_eq!(lmstudio.provider(), LlmProvider::Lmstudio);
        assert_eq!(lmstudio.default_model, LMSTUDIO_DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn two_openai_compatible_providers_coexist_in_one_gateway() {
        // The whole point of the configurable provider slot: register OpenAI and
        // OpenRouter side by side and route each request to its own adapter.
        let (openai_port, _o) = recording_port(json!({
            "choices": [{ "message": { "content": "from-openai" }, "finish_reason": "stop" }]
        }));
        let (router_port, _r) = recording_port(json!({
            "choices": [{ "message": { "content": "from-openrouter" }, "finish_reason": "stop" }]
        }));

        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::with_provider(
            openai_port,
            LlmProvider::Openai,
            OPENAI_DEFAULT_MODEL,
        )));
        gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::with_provider(
            router_port,
            LlmProvider::Openrouter,
            OPENROUTER_DEFAULT_MODEL,
        )));

        let openai = gateway
            .complete(LlmRequest {
                provider: LlmProvider::Openai,
                ..base_request()
            })
            .await
            .unwrap();
        let router = gateway
            .complete(LlmRequest {
                provider: LlmProvider::Openrouter,
                ..base_request()
            })
            .await
            .unwrap();

        assert_eq!(openai.content, "from-openai");
        assert_eq!(openai.provider, LlmProvider::Openai);
        assert_eq!(router.content, "from-openrouter");
        assert_eq!(router.provider, LlmProvider::Openrouter);
    }

    #[tokio::test]
    async fn keeps_a_namespaced_model_id_verbatim() {
        let (port, bodies) = recording_port(text_response());
        let adapter = OpenAiCompatibleAdapter::with_provider(
            port,
            LlmProvider::Openrouter,
            OPENROUTER_DEFAULT_MODEL,
        );

        let request = LlmRequest {
            provider: LlmProvider::Openrouter,
            model: "anthropic/claude-3.5-sonnet".to_owned(),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        // A `/`-namespaced id is explicit and must not be rewritten to the default,
        // even though it contains `claude-`.
        assert_eq!(
            bodies.lock().unwrap()[0]["model"],
            json!("anthropic/claude-3.5-sonnet")
        );
    }

    #[test]
    fn empty_tool_call_arguments_default_to_an_empty_object() {
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("not json"), json!({}));
        assert_eq!(parse_arguments("{\"a\":1}"), json!({ "a": 1 }));
    }

    #[test]
    fn deserializes_the_real_wire_response_shape() {
        let raw: OpenAiChatResponse = serde_json::from_str(
            r#"{
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "model": "mistral-small-latest",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hi there.",
                        "tool_calls": [{
                            "id": "call_01",
                            "type": "function",
                            "function": { "name": "search", "arguments": "{\"q\":\"x\"}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14 }
            }"#,
        )
        .unwrap();

        let choice = &raw.choices[0];
        assert_eq!(choice.message.content.as_deref(), Some("Hi there."));
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let tool_calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls[0].function.name, "search");
        assert_eq!(raw.usage.as_ref().unwrap().prompt_tokens, Some(10));
    }

    /// ADR 0033 phase 13: the SSE accumulator reassembles a streamed chat-completion —
    /// content deltas in order, usage from the trailing chunk, mapped to the same response.
    #[test]
    fn stream_accumulator_reassembles_a_text_completion() {
        let mut acc = OpenAiStreamAccumulator::default();
        let deltas: Vec<String> = [
            r#"{"choices":[{"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"content":"Hel"},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":2}}"#,
            "[DONE]",
        ]
        .iter()
        .filter_map(|e| acc.push_event(e))
        .collect();
        assert_eq!(deltas, vec!["Hel".to_owned(), "lo".to_owned()]);

        let raw = acc.finish();
        let request = LlmRequest {
            provider: LlmProvider::Openai,
            model: "gpt-x".to_owned(),
            messages: vec![],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        };
        let response = to_response(
            &request,
            "gpt-x".to_owned(),
            serde_json::from_value(raw).unwrap(),
        );
        assert_eq!(response.content, "Hello");
        assert_eq!(response.stop_reason.as_deref(), Some("stop"));
        assert_eq!(response.usage.prompt_tokens, 9);
        assert_eq!(response.usage.completion_tokens, 2);
    }

    /// Streamed `tool_calls` arrive as fragments (id+name on the first, `arguments` split
    /// across chunks); the accumulator concatenates them into one structured call.
    #[test]
    fn stream_accumulator_reassembles_streamed_tool_calls() {
        let mut acc = OpenAiStreamAccumulator::default();
        for event in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":""}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"rust\"}"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ] {
            assert_eq!(acc.push_event(event), None);
        }
        let raw = acc.finish();
        let request = LlmRequest {
            provider: LlmProvider::Openai,
            model: "gpt-x".to_owned(),
            messages: vec![],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        };
        let response = to_response(
            &request,
            "gpt-x".to_owned(),
            serde_json::from_value(raw).unwrap(),
        );
        let calls = response.tool_calls.expect("tool calls assembled");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].input, json!({ "q": "rust" }));
    }
}
