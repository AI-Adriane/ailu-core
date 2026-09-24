//! Anthropic provider adapter — the Rust port of `@adriane-ai/llm-gateway`'s
//! `anthropic-adapter.ts`, behavior-for-behavior.
//!
//! The adapter talks to the Messages API through a single seam, [`AnthropicPort`],
//! so the cache + accounting logic is fully covered by tests without a network
//! call or an API key. [`HttpAnthropicPort`] is the real reqwest-based
//! implementation behind the same trait.
//!
//! Request assembly mirrors the TS adapter exactly:
//! - the cacheable prefix is `tools` then `system` (Anthropic render order); a
//!   breakpoint on the **last** tool caches the whole deterministic tool list and
//!   one on the single system block caches the system prefix,
//! - `req.system` plus any system-role messages are folded into one system block
//!   (joined with a blank line), and system-role messages are removed from the
//!   message list,
//! - a model not starting with `claude-` resolves to the default model,
//! - sampling params (`temperature`, …) are intentionally dropped — recent Opus
//!   models reject them.
//!
//! Tool transcript (ADR 0014): assistant `tool_calls` serialize to `tool_use` content
//! blocks and `role:"tool"` results to a `tool_result` block (linked by `tool_use_id`), so
//! a tool-calling agent holds a real multi-turn conversation with Anthropic.
//!
//! Streaming (ADR 0033): [`AnthropicPort::create_stream`] streams the Messages API SSE,
//! reassembled by [`AnthropicStreamAccumulator`] (unit-tested offline); the assembled
//! response is byte-identical to `create()`.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::LlmError;
use crate::gateway::{LlmProviderAdapter, TokenSink};
use crate::sse::SseDecoder;
use crate::types::{
    ContentBlock, LlmProvider, LlmRequest, LlmResponse, LlmToolCall, LlmUsage, MediaSource,
    ResponseFormat,
};

/// Model used when the request does not name a Claude model.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";
/// Default `max_tokens` for non-streaming completions (mirrors the TS adapter).
pub const DEFAULT_MAX_TOKENS: u32 = 16000;

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Provider-shaped request (the cache seam)
// ---------------------------------------------------------------------------

/// One system text block. `cacheable: true` becomes a `cache_control`
/// breakpoint on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemBlock {
    pub text: String,
    pub cacheable: bool,
}

/// One tool definition. Only the last tool in the list carries
/// `cacheable: true` — that single breakpoint caches the whole tool list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolParam {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    pub cacheable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    /// Either a plain string or a content-block array (`tool_use` / `tool_result`).
    pub content: Value,
}

/// Provider-shaped request the adapter assembles. This is the cache seam: the
/// `system` and `tools` blocks carry the cache breakpoints and must stay
/// byte-stable across calls. The HTTP port translates it into the wire body;
/// tests fake the port and assert on this shape directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicCreateParams {
    pub model: String,
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolParam>>,
    /// ADR 0029: forces a specific tool (`{ "type": "tool", "name": … }`) — Anthropic's
    /// only route to schema-constrained output, since it has no `response_format`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    pub messages: Vec<AnthropicMessage>,
}

// ---------------------------------------------------------------------------
// Raw wire response (snake_case, matches Anthropic's real JSON)
// ---------------------------------------------------------------------------

/// One content block of the raw Messages API response. Only the fields the
/// adapter reads; everything is optional except the discriminant.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AnthropicContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
}

/// Structural subset of the Messages API response the adapter actually reads.
/// Deserializes straight from Anthropic's wire JSON (snake_case).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AnthropicRawResponse {
    pub content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

/// Reassembles a streaming Messages response from its SSE events (ADR 0033, phase 13).
/// Pure + offline-testable: feed each event's `data:` JSON to [`Self::push_event`] (which
/// returns a text delta when the event carries one), then [`Self::finish`] yields the same
/// [`AnthropicRawResponse`] the non-streaming `create()` would have produced — so the
/// assembled response (the authoritative one the agent consumes) is correct regardless of
/// how the deltas were chunked.
///
/// Anthropic's event grammar (the fields we read): `message_start` carries the input/cache
/// usage; `content_block_start` opens a `text` or `tool_use` block at an index;
/// `content_block_delta` carries a `text_delta` (a token) or an `input_json_delta` (a
/// fragment of a tool call's JSON input); `message_delta` carries the `stop_reason` and the
/// output token count; `message_stop`/`ping` are terminal/no-ops.
#[derive(Default)]
pub struct AnthropicStreamAccumulator {
    blocks: Vec<AnthropicContentBlock>,
    /// Per-block accumulated `tool_use` input JSON fragments, indexed alongside `blocks`.
    tool_json: Vec<String>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

impl AnthropicStreamAccumulator {
    fn ensure_block(&mut self, index: usize) {
        while self.blocks.len() <= index {
            self.blocks.push(AnthropicContentBlock::default());
            self.tool_json.push(String::new());
        }
    }

    /// Process one SSE `data:` payload. Returns the text delta if this event carried one.
    pub fn push_event(&mut self, data: &str) -> Option<String> {
        let value: Value = serde_json::from_str(data).ok()?;
        match value.get("type").and_then(Value::as_str)? {
            "message_start" => {
                if let Some(usage) = value.pointer("/message/usage") {
                    self.merge_usage(usage);
                }
                None
            }
            "content_block_start" => {
                let index = value.get("index").and_then(Value::as_u64)? as usize;
                self.ensure_block(index);
                if let Some(block) = value.get("content_block") {
                    let entry = &mut self.blocks[index];
                    entry.block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    entry.id = block.get("id").and_then(Value::as_str).map(str::to_owned);
                    entry.name = block.get("name").and_then(Value::as_str).map(str::to_owned);
                }
                None
            }
            "content_block_delta" => {
                let index = value.get("index").and_then(Value::as_u64)? as usize;
                self.ensure_block(index);
                let delta = value.get("delta")?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = delta.get("text").and_then(Value::as_str)?;
                        let entry = &mut self.blocks[index];
                        entry.text.get_or_insert_with(String::new).push_str(text);
                        Some(text.to_owned())
                    }
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) {
                            self.tool_json[index].push_str(fragment);
                        }
                        None
                    }
                    _ => None,
                }
            }
            "message_delta" => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_owned());
                }
                if let Some(usage) = value.get("usage") {
                    self.merge_usage(usage);
                }
                None
            }
            _ => None,
        }
    }

    fn merge_usage(&mut self, usage: &Value) {
        if let Some(v) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.usage.input_tokens = v as u32;
        }
        if let Some(v) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = v as u32;
        }
        if let Some(v) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.usage.cache_read_input_tokens = Some(v as u32);
        }
        if let Some(v) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.usage.cache_creation_input_tokens = Some(v as u32);
        }
    }

    /// Finalize: parse each `tool_use` block's accumulated JSON into its `input`, and emit
    /// the structural [`AnthropicRawResponse`] the rest of the adapter maps via `to_response`.
    pub fn finish(mut self) -> AnthropicRawResponse {
        for (index, block) in self.blocks.iter_mut().enumerate() {
            if block.block_type == "tool_use" {
                let raw = &self.tool_json[index];
                block.input = if raw.is_empty() {
                    Some(Value::Object(Map::new()))
                } else {
                    Some(serde_json::from_str(raw).unwrap_or(Value::Object(Map::new())))
                };
            }
        }
        AnthropicRawResponse {
            content: self.blocks,
            stop_reason: self.stop_reason,
            usage: self.usage,
        }
    }
}

// ---------------------------------------------------------------------------
// Port seam
// ---------------------------------------------------------------------------

/// The only seam onto the real Anthropic API. Production uses
/// [`HttpAnthropicPort`]; tests supply a fake so no network is ever touched.
#[async_trait]
pub trait AnthropicPort: Send + Sync {
    async fn create(&self, params: AnthropicCreateParams)
        -> Result<AnthropicRawResponse, LlmError>;

    /// Stream the Messages response, calling `on_delta` per text delta, returning the
    /// fully-assembled raw response (ADR 0033, phase 13). Default impl is chunk-once: it
    /// calls [`Self::create`] and emits the assembled text as a single delta — so a test
    /// port (or any non-HTTP port) streams without implementing real SSE. Only
    /// [`HttpAnthropicPort`] overrides this with the provider's event stream.
    async fn create_stream(
        &self,
        params: AnthropicCreateParams,
        on_delta: &TokenSink<'_>,
    ) -> Result<AnthropicRawResponse, LlmError> {
        let raw = self.create(params).await?;
        let text: String = raw
            .content
            .iter()
            .filter(|block| block.block_type == "text")
            .filter_map(|block| block.text.clone())
            .collect();
        if !text.is_empty() {
            on_delta(&text);
        }
        Ok(raw)
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

pub struct AnthropicAdapter {
    port: Box<dyn AnthropicPort>,
    default_model: String,
}

impl AnthropicAdapter {
    /// Build an adapter over the given port with the standard default model.
    pub fn new(port: Box<dyn AnthropicPort>) -> Self {
        Self::with_default_model(port, DEFAULT_MODEL)
    }

    /// Build an adapter over the given port, overriding the fallback model used
    /// when a request names a non-Claude model.
    pub fn with_default_model(
        port: Box<dyn AnthropicPort>,
        default_model: impl Into<String>,
    ) -> Self {
        AnthropicAdapter {
            port,
            default_model: default_model.into(),
        }
    }

    /// Convenience constructor wiring the real HTTP port from
    /// `ANTHROPIC_API_KEY`. Errors when the variable is unset.
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LlmError::Provider("ANTHROPIC_API_KEY is not set".to_owned()))?;
        Ok(Self::new(Box::new(HttpAnthropicPort::new(api_key))))
    }

    /// Assemble the provider request. See the module docs for the cache layout.
    /// Sampling params are intentionally dropped; no date/timestamp is added.
    fn build_params(&self, req: &LlmRequest) -> AnthropicCreateParams {
        let system_text = collect_system(req);

        let messages = req
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                // Tool result → a `tool_result` content block (role user) linked by tool_use_id.
                if m.role == "tool" {
                    return AnthropicMessage {
                        role: AnthropicRole::User,
                        content: json!([{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content,
                        }]),
                    };
                }
                let role = if m.role == "assistant" {
                    AnthropicRole::Assistant
                } else {
                    AnthropicRole::User
                };
                // Assistant tool calls → `tool_use` blocks (+ a leading text block if any).
                if let Some(calls) = &m.tool_calls {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(json!({ "type": "text", "text": m.content }));
                    }
                    for call in calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.input,
                        }));
                    }
                    return AnthropicMessage {
                        role,
                        content: Value::Array(blocks),
                    };
                }
                // ADR 0030: multimodal — content blocks become an Anthropic content array.
                if let Some(content_blocks) = &m.content_blocks {
                    return AnthropicMessage {
                        role,
                        content: Value::Array(anthropic_content_blocks(content_blocks)),
                    };
                }
                AnthropicMessage {
                    role,
                    content: Value::String(m.content.clone()),
                }
            })
            .collect();

        let system = if system_text.is_empty() {
            None
        } else {
            Some(vec![SystemBlock {
                text: system_text,
                cacheable: true,
            }])
        };

        let mut tools: Vec<ToolParam> = match &req.tools {
            Some(tools) if !tools.is_empty() => {
                let last = tools.len() - 1;
                tools
                    .iter()
                    .enumerate()
                    .map(|(index, tool)| ToolParam {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                        // Breakpoint on the last tool caches the whole list.
                        cacheable: index == last,
                    })
                    .collect()
            }
            _ => Vec::new(),
        };

        // ADR 0029: Anthropic has no `response_format`; force a synthetic tool whose
        // `input_schema` IS the output schema. Appended AFTER any real tools'
        // cache breakpoint (cacheable: false) so it never busts the cached tool prefix;
        // when it is the only tool it carries the breakpoint itself (the schema is
        // call-stable, so caching it is safe).
        let mut tool_choice = None;
        if let Some(ResponseFormat::JsonSchema { name, schema, .. }) = &req.response_format {
            let only = tools.is_empty();
            tools.push(ToolParam {
                name: name.clone(),
                description: Some(
                    "Return the final result by calling this tool with arguments matching its schema."
                        .to_owned(),
                ),
                input_schema: schema.clone(),
                cacheable: only,
            });
            tool_choice = Some(json!({ "type": "tool", "name": name }));
        }

        let tools = if tools.is_empty() { None } else { Some(tools) };

        AnthropicCreateParams {
            model: self.resolve_model(&req.model),
            max_tokens: req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system,
            tools,
            tool_choice,
            messages,
        }
    }

    fn resolve_model(&self, model: &str) -> String {
        if model.starts_with("claude-") {
            model.to_owned()
        } else {
            self.default_model.clone()
        }
    }
}

/// `req.system` plus any system-role messages, joined with a blank line.
fn collect_system(req: &LlmRequest) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(system) = &req.system {
        if !system.is_empty() {
            parts.push(system);
        }
    }
    for message in &req.messages {
        if message.role == "system" {
            parts.push(&message.content);
        }
    }
    parts.join("\n\n")
}

fn to_response(request: &LlmRequest, model: String, raw: AnthropicRawResponse) -> LlmResponse {
    let content: String = raw
        .content
        .iter()
        .filter(|block| block.block_type == "text")
        .map(|block| block.text.clone().unwrap_or_default())
        .collect();

    let tool_calls: Vec<LlmToolCall> = raw
        .content
        .into_iter()
        .filter(|block| block.block_type == "tool_use")
        .map(|block| LlmToolCall {
            id: block.id.unwrap_or_default(),
            name: block.name.unwrap_or_default(),
            input: block.input.unwrap_or_else(|| Value::Object(Map::new())),
        })
        .collect();

    LlmResponse {
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        stop_reason: raw.stop_reason,
        usage: LlmUsage {
            prompt_tokens: raw.usage.input_tokens,
            completion_tokens: raw.usage.output_tokens,
            cache_read_tokens: Some(raw.usage.cache_read_input_tokens.unwrap_or(0)),
            cache_write_tokens: Some(raw.usage.cache_creation_input_tokens.unwrap_or(0)),
        },
        model,
        provider: request.provider,
        content_blocks: None,
    }
}

#[async_trait]
impl LlmProviderAdapter for AnthropicAdapter {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Anthropic
    }

    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let params = self.build_params(&request);
        let model = params.model.clone();
        let raw = self.port.create(params).await?;
        Ok(to_response(&request, model, raw))
    }

    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let params = self.build_params(&request);
        let model = params.model.clone();
        // The assembled raw response is authoritative — `to_response` maps it exactly as
        // on the `complete()` path, so the agent's result is identical regardless of chunking.
        let raw = self.port.create_stream(params, on_delta).await?;
        Ok(to_response(&request, model, raw))
    }
}

// ---------------------------------------------------------------------------
// HTTP port (the only code that touches the network)
// ---------------------------------------------------------------------------

/// Real [`AnthropicPort`] over `POST /v1/messages`. Never exercised in tests —
/// the pure [`build_request_body`] carries the wire-shape logic and is
/// unit-tested instead.
pub struct HttpAnthropicPort {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl HttpAnthropicPort {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, ANTHROPIC_BASE_URL)
    }

    /// Override the API host (e.g. a local stub during manual testing).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        HttpAnthropicPort {
            client: crate::http::http_client(),
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }
}

/// Build the Messages API request body. `cacheable: true` becomes
/// `"cache_control": {"type": "ephemeral"}` on that block; tool `input_schema`
/// is emitted with `"type": "object"` merged in (schema keys win), exactly like
/// the TS port.
/// ADR 0030: map content blocks to Anthropic content blocks. Image → `image` block,
/// File → `document` block (base64 or url source). Audio is unsupported by Anthropic
/// messages and is skipped; an unresolved Artifact source yields no block.
fn anthropic_content_blocks(blocks: &[ContentBlock]) -> Vec<Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(json!({ "type": "text", "text": text })),
            ContentBlock::Image { source } => {
                anthropic_source(source).map(|s| json!({ "type": "image", "source": s }))
            }
            ContentBlock::File { source } => {
                anthropic_source(source).map(|s| json!({ "type": "document", "source": s }))
            }
            ContentBlock::Audio { .. } => None,
        })
        .collect()
}

fn anthropic_source(source: &MediaSource) -> Option<Value> {
    if let Some((media_type, data)) = source.as_base64() {
        Some(json!({ "type": "base64", "media_type": media_type, "data": data }))
    } else if let MediaSource::Url { url, .. } = source {
        Some(json!({ "type": "url", "url": url }))
    } else {
        None // unresolved artifact — resolved upstream by the gateway (ADR 0030 9c)
    }
}

pub fn build_request_body(params: &AnthropicCreateParams) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(params.model));
    body.insert("max_tokens".to_owned(), json!(params.max_tokens));
    body.insert(
        "messages".to_owned(),
        Value::Array(
            params
                .messages
                .iter()
                .map(|m| json!({ "role": m.role, "content": m.content }))
                .collect(),
        ),
    );

    if let Some(system) = &params.system {
        body.insert(
            "system".to_owned(),
            Value::Array(
                system
                    .iter()
                    .map(|block| {
                        let mut wire = Map::new();
                        wire.insert("type".to_owned(), json!("text"));
                        wire.insert("text".to_owned(), json!(block.text));
                        if block.cacheable {
                            wire.insert("cache_control".to_owned(), json!({ "type": "ephemeral" }));
                        }
                        Value::Object(wire)
                    })
                    .collect(),
            ),
        );
    }

    if let Some(tools) = &params.tools {
        body.insert(
            "tools".to_owned(),
            Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        let mut schema = Map::new();
                        schema.insert("type".to_owned(), json!("object"));
                        if let Value::Object(map) = &tool.input_schema {
                            for (key, value) in map {
                                schema.insert(key.clone(), value.clone());
                            }
                        }
                        let mut wire = Map::new();
                        wire.insert("name".to_owned(), json!(tool.name));
                        if let Some(description) = &tool.description {
                            wire.insert("description".to_owned(), json!(description));
                        }
                        wire.insert("input_schema".to_owned(), Value::Object(schema));
                        if tool.cacheable {
                            wire.insert("cache_control".to_owned(), json!({ "type": "ephemeral" }));
                        }
                        Value::Object(wire)
                    })
                    .collect(),
            ),
        );
    }

    // ADR 0029: forced-tool route to schema-constrained output.
    if let Some(tool_choice) = &params.tool_choice {
        body.insert("tool_choice".to_owned(), tool_choice.clone());
    }

    Value::Object(body)
}

#[async_trait]
impl AnthropicPort for HttpAnthropicPort {
    async fn create(
        &self,
        params: AnthropicCreateParams,
    ) -> Result<AnthropicRawResponse, LlmError> {
        let body = build_request_body(&params);
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|err| LlmError::Provider(format!("anthropic request failed: {err}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!(
                "anthropic returned {status}: {text}"
            )));
        }

        response
            .json::<AnthropicRawResponse>()
            .await
            .map_err(|err| LlmError::Provider(format!("anthropic response decode failed: {err}")))
    }

    /// Real SSE over `POST /v1/messages` with `"stream": true`. Consumes the byte stream,
    /// frames it into events, and folds them through [`AnthropicStreamAccumulator`], emitting
    /// each text delta live. The transport (reqwest `bytes_stream`) is verified on a live key
    /// via `scripts/probe-anthropic.ts`, not in CI — like `create()`; the SSE reassembly is
    /// unit-tested offline through the accumulator.
    async fn create_stream(
        &self,
        params: AnthropicCreateParams,
        on_delta: &TokenSink<'_>,
    ) -> Result<AnthropicRawResponse, LlmError> {
        let mut body = build_request_body(&params);
        if let Value::Object(map) = &mut body {
            map.insert("stream".to_owned(), json!(true));
        }
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|err| LlmError::Provider(format!("anthropic stream request failed: {err}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!(
                "anthropic returned {status}: {text}"
            )));
        }

        let mut decoder = SseDecoder::default();
        let mut accumulator = AnthropicStreamAccumulator::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|err| {
                LlmError::Provider(format!("anthropic stream read failed: {err}"))
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
    use crate::types::{LlmMessage, LlmToolDef, ResponseFormat};

    struct RecordingPort {
        calls: Arc<Mutex<Vec<AnthropicCreateParams>>>,
        response: AnthropicRawResponse,
    }

    #[async_trait]
    impl AnthropicPort for RecordingPort {
        async fn create(
            &self,
            params: AnthropicCreateParams,
        ) -> Result<AnthropicRawResponse, LlmError> {
            self.calls.lock().unwrap().push(params);
            Ok(self.response.clone())
        }
    }

    /// Captures the params the adapter builds and returns a canned response.
    fn recording_port(
        response: AnthropicRawResponse,
    ) -> (
        Box<dyn AnthropicPort>,
        Arc<Mutex<Vec<AnthropicCreateParams>>>,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let port = RecordingPort {
            calls: Arc::clone(&calls),
            response,
        };
        (Box::new(port), calls)
    }

    fn text_response() -> AnthropicRawResponse {
        AnthropicRawResponse {
            content: vec![AnthropicContentBlock {
                block_type: "text".to_owned(),
                text: Some("hello".to_owned()),
                ..AnthropicContentBlock::default()
            }],
            stop_reason: None,
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(0),
            },
        }
    }

    fn base_request() -> LlmRequest {
        LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "claude-opus-4-8".to_owned(),
            messages: vec![LlmMessage::text("user", "Hi")],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        }
    }

    #[tokio::test]
    async fn response_format_forces_a_synthetic_tool_without_busting_the_cache_prefix() {
        // ADR 0029: Anthropic has no `response_format`; the adapter appends a synthetic
        // schema-tool + `tool_choice`. It must sit AFTER the real tools' cache breakpoint.
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        let request = LlmRequest {
            tools: Some(vec![LlmToolDef {
                name: "search".to_owned(),
                description: None,
                input_schema: json!({ "type": "object" }),
            }]),
            response_format: Some(ResponseFormat::JsonSchema {
                name: "Verdict".to_owned(),
                schema: json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } }),
                strict: true,
            }),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let calls = calls.lock().unwrap();
        let params = &calls[0];
        let tools = params.tools.as_ref().unwrap();
        // Real tool first (keeps the breakpoint), synthetic schema-tool appended last.
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search");
        assert!(tools[0].cacheable, "real tool keeps the cache breakpoint");
        assert_eq!(tools[1].name, "Verdict");
        assert!(
            !tools[1].cacheable,
            "synthetic tool must not bust the cached prefix"
        );
        // tool_choice forces the synthetic tool.
        assert_eq!(
            params.tool_choice,
            Some(json!({ "type": "tool", "name": "Verdict" }))
        );
        // And it survives to the wire body.
        let body = build_request_body(params);
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "tool", "name": "Verdict" })
        );
    }

    #[tokio::test]
    async fn response_format_alone_makes_the_synthetic_tool_the_only_cacheable_one() {
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        let request = LlmRequest {
            response_format: Some(ResponseFormat::JsonSchema {
                name: "Out".to_owned(),
                schema: json!({ "type": "object" }),
                strict: false,
            }),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let calls = calls.lock().unwrap();
        let tools = calls[0].tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "Out");
        assert!(tools[0].cacheable, "the only tool carries the breakpoint");
    }

    #[tokio::test]
    async fn image_content_block_maps_to_an_anthropic_image_block() {
        // ADR 0030: content blocks → an Anthropic content array (image source base64).
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);
        let request = LlmRequest {
            messages: vec![LlmMessage::with_blocks(
                "user",
                vec![
                    ContentBlock::Text {
                        text: "describe".to_owned(),
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

        let calls = calls.lock().unwrap();
        let msg = calls[0]
            .messages
            .iter()
            .find(|m| m.content.is_array())
            .unwrap();
        let blocks = msg.content.as_array().unwrap();
        assert_eq!(blocks[0], json!({ "type": "text", "text": "describe" }));
        assert_eq!(blocks[1]["type"], json!("image"));
        assert_eq!(blocks[1]["source"]["type"], json!("base64"));
        assert_eq!(blocks[1]["source"]["media_type"], json!("image/png"));
        assert_eq!(blocks[1]["source"]["data"], json!("AAAA"));
    }

    #[tokio::test]
    async fn marks_the_system_block_and_the_last_tool_as_cacheable() {
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        let request = LlmRequest {
            system: Some("You are a helpful agent.".to_owned()),
            tools: Some(vec![
                LlmToolDef {
                    name: "search".to_owned(),
                    description: None,
                    input_schema: json!({ "query": { "type": "string" } }),
                },
                LlmToolDef {
                    name: "fetch".to_owned(),
                    description: None,
                    input_schema: json!({ "url": { "type": "string" } }),
                },
            ]),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let calls = calls.lock().unwrap();
        let params = &calls[0];
        let system = params.system.as_ref().unwrap();
        assert!(system[0].cacheable);
        // Only the last tool carries the breakpoint — it caches the whole list.
        let flags: Vec<bool> = params
            .tools
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.cacheable)
            .collect();
        assert_eq!(flags, vec![false, true]);
    }

    #[tokio::test]
    async fn maps_usage_including_cache_read_and_write_tokens() {
        let (port, _calls) = recording_port(AnthropicRawResponse {
            usage: AnthropicUsage {
                input_tokens: 12,
                output_tokens: 8,
                cache_read_input_tokens: Some(2048),
                cache_creation_input_tokens: Some(512),
            },
            ..text_response()
        });
        let adapter = AnthropicAdapter::new(port);

        let result = adapter.complete(base_request()).await.unwrap();

        assert_eq!(
            result.usage,
            LlmUsage {
                prompt_tokens: 12,
                completion_tokens: 8,
                cache_read_tokens: Some(2048),
                cache_write_tokens: Some(512),
            }
        );
        assert_eq!(result.content, "hello");
        assert_eq!(result.provider, LlmProvider::Anthropic);
    }

    #[tokio::test]
    async fn treats_missing_cache_usage_as_zero() {
        let (port, _calls) = recording_port(AnthropicRawResponse {
            usage: AnthropicUsage {
                input_tokens: 5,
                output_tokens: 5,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            ..text_response()
        });
        let adapter = AnthropicAdapter::new(port);

        let result = adapter.complete(base_request()).await.unwrap();

        assert_eq!(result.usage.cache_read_tokens, Some(0));
        assert_eq!(result.usage.cache_write_tokens, Some(0));
    }

    #[tokio::test]
    async fn falls_back_to_the_default_model_when_the_request_model_is_a_placeholder() {
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::with_default_model(port, "claude-opus-4-8");

        let request = LlmRequest {
            model: "react-agent".to_owned(),
            ..base_request()
        };
        let result = adapter.complete(request).await.unwrap();

        assert_eq!(calls.lock().unwrap()[0].model, "claude-opus-4-8");
        assert_eq!(result.model, "claude-opus-4-8");
    }

    #[tokio::test]
    async fn keeps_an_explicit_claude_model_and_folds_system_messages_into_the_system_prefix() {
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        let request = LlmRequest {
            model: "claude-haiku-4-5".to_owned(),
            system: Some("Base.".to_owned()),
            messages: vec![
                LlmMessage::text("system", "Extra rule."),
                LlmMessage::text("user", "Go"),
            ],
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let calls = calls.lock().unwrap();
        let params = &calls[0];
        assert_eq!(params.model, "claude-haiku-4-5");
        assert_eq!(
            params.system.as_ref().unwrap()[0].text,
            "Base.\n\nExtra rule."
        );
        // System-role messages are pulled out of the message list.
        assert_eq!(
            params.messages,
            vec![AnthropicMessage {
                role: AnthropicRole::User,
                content: json!("Go"),
            }]
        );
    }

    #[tokio::test]
    async fn surfaces_tool_use_blocks_as_structured_tool_calls_and_the_stop_reason() {
        let (port, _calls) = recording_port(AnthropicRawResponse {
            stop_reason: Some("tool_use".to_owned()),
            content: vec![
                AnthropicContentBlock {
                    block_type: "text".to_owned(),
                    text: Some("Let me search.".to_owned()),
                    ..AnthropicContentBlock::default()
                },
                AnthropicContentBlock {
                    block_type: "tool_use".to_owned(),
                    id: Some("tu_1".to_owned()),
                    name: Some("search".to_owned()),
                    input: Some(json!({ "query": "adriane" })),
                    ..AnthropicContentBlock::default()
                },
            ],
            ..text_response()
        });
        let adapter = AnthropicAdapter::new(port);

        let response = adapter.complete(base_request()).await.unwrap();

        assert_eq!(response.content, "Let me search.");
        assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(
            response.tool_calls,
            Some(vec![LlmToolCall {
                id: "tu_1".to_owned(),
                name: "search".to_owned(),
                input: json!({ "query": "adriane" }),
            }])
        );
    }

    #[tokio::test]
    async fn omits_tool_calls_when_the_model_returns_only_text() {
        let (port, _calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        let response = adapter.complete(base_request()).await.unwrap();

        assert_eq!(response.tool_calls, None);
    }

    #[tokio::test]
    async fn uses_the_default_max_tokens_unless_the_request_overrides_it() {
        let (port, calls) = recording_port(text_response());
        let adapter = AnthropicAdapter::new(port);

        adapter.complete(base_request()).await.unwrap();
        let request = LlmRequest {
            max_tokens: Some(512),
            ..base_request()
        };
        adapter.complete(request).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(calls[1].max_tokens, 512);
    }

    #[tokio::test]
    async fn integrates_through_the_default_gateway_routing() {
        let (port, _calls) = recording_port(text_response());
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(AnthropicAdapter::new(port)));

        let result = gateway.complete(base_request()).await.unwrap();

        assert_eq!(result.content, "hello");
        assert_eq!(result.provider, LlmProvider::Anthropic);
    }

    #[test]
    fn deserializes_the_real_wire_response_shape() {
        let raw: AnthropicRawResponse = serde_json::from_str(
            r#"{
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-4-8",
                "content": [
                    {"type": "text", "text": "Hi there."},
                    {"type": "tool_use", "id": "toolu_01", "name": "search", "input": {"query": "x"}}
                ],
                "stop_reason": "tool_use",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "cache_read_input_tokens": 1024,
                    "cache_creation_input_tokens": 0
                }
            }"#,
        )
        .unwrap();

        assert_eq!(raw.content.len(), 2);
        assert_eq!(raw.content[0].text.as_deref(), Some("Hi there."));
        assert_eq!(raw.content[1].block_type, "tool_use");
        assert_eq!(raw.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(raw.usage.cache_read_input_tokens, Some(1024));
    }

    #[test]
    fn builds_the_wire_body_with_cache_control_on_system_and_last_tool_only() {
        let params = AnthropicCreateParams {
            model: "claude-opus-4-8".to_owned(),
            max_tokens: 16000,
            system: Some(vec![SystemBlock {
                text: "Be helpful.".to_owned(),
                cacheable: true,
            }]),
            tools: Some(vec![
                ToolParam {
                    name: "search".to_owned(),
                    description: Some("Search things".to_owned()),
                    input_schema: json!({ "properties": { "query": { "type": "string" } } }),
                    cacheable: false,
                },
                ToolParam {
                    name: "fetch".to_owned(),
                    description: None,
                    input_schema: json!({ "type": "custom", "properties": {} }),
                    cacheable: true,
                },
            ]),
            tool_choice: None,
            messages: vec![AnthropicMessage {
                role: AnthropicRole::User,
                content: json!("Hi"),
            }],
        };

        let body = build_request_body(&params);

        assert_eq!(body["model"], json!("claude-opus-4-8"));
        assert_eq!(body["max_tokens"], json!(16000));
        assert_eq!(
            body["messages"],
            json!([{ "role": "user", "content": "Hi" }])
        );

        // System block carries the ephemeral breakpoint.
        assert_eq!(
            body["system"],
            json!([{
                "type": "text",
                "text": "Be helpful.",
                "cache_control": { "type": "ephemeral" }
            }])
        );

        // input_schema gets "type": "object" merged in (schema keys win); only
        // the last tool carries cache_control.
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[0]["input_schema"]["type"], json!("object"));
        assert_eq!(
            tools[0]["input_schema"]["properties"]["query"]["type"],
            json!("string")
        );
        assert_eq!(tools[0]["description"], json!("Search things"));
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["input_schema"]["type"], json!("custom"));
        assert!(tools[1].get("description").is_none());
        assert_eq!(tools[1]["cache_control"], json!({ "type": "ephemeral" }));
    }

    /// ADR 0033 phase 13: the SSE accumulator reassembles a text stream — the deltas arrive
    /// in order and the assembled response carries the concatenated text + usage.
    #[test]
    fn stream_accumulator_reassembles_a_text_response() {
        let mut acc = AnthropicStreamAccumulator::default();
        let events = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"cache_read_input_tokens":4}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let deltas: Vec<String> = events.iter().filter_map(|e| acc.push_event(e)).collect();
        assert_eq!(deltas, vec!["Hel".to_owned(), "lo".to_owned()]);

        let raw = acc.finish();
        assert_eq!(raw.content.len(), 1);
        assert_eq!(raw.content[0].text.as_deref(), Some("Hello"));
        assert_eq!(raw.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(raw.usage.input_tokens, 12);
        assert_eq!(raw.usage.output_tokens, 7);
        assert_eq!(raw.usage.cache_read_input_tokens, Some(4));

        // The assembled raw maps to the same LlmResponse shape `complete()` produces.
        let request = LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "claude-opus-4-8".to_owned(),
            messages: vec![],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        };
        let response = to_response(&request, "claude-opus-4-8".to_owned(), raw);
        assert_eq!(response.content, "Hello");
        assert_eq!(response.usage.completion_tokens, 7);
    }

    /// A `tool_use` block's input JSON arrives as `input_json_delta` fragments; the
    /// accumulator concatenates and parses them into the structured tool call.
    #[test]
    fn stream_accumulator_reassembles_a_tool_use_from_json_fragments() {
        let mut acc = AnthropicStreamAccumulator::default();
        for event in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu1","name":"search"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"rust\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ] {
            // tool_use fragments emit no text delta.
            assert_eq!(acc.push_event(event), None);
        }
        let raw = acc.finish();
        assert_eq!(raw.content[0].block_type, "tool_use");
        assert_eq!(raw.content[0].id.as_deref(), Some("tu1"));
        assert_eq!(raw.content[0].name.as_deref(), Some("search"));
        assert_eq!(raw.content[0].input, Some(json!({ "q": "rust" })));
    }
}
