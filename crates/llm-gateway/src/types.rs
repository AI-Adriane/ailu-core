//! LLM request/response types. Wire-compatible (camelCase) with the TS gateway.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Openai,
    Anthropic,
    Mistral,
    Ollama,
    Google,
    Minimax,
    Openrouter,
    Huggingface,
    Lmstudio,
    Mock,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmMessage {
    /// `"system" | "user" | "assistant" | "tool"`.
    pub role: String,
    pub content: String,
    /// Native tool calls the assistant emitted — set on `assistant` history entries so the
    /// provider sees a coherent function-calling transcript (no redundant re-calls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    /// Links a `role:"tool"` result back to the assistant tool call that produced it
    /// (the OpenAI / Anthropic id; Anthropic's `tool_result.tool_use_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Name of the tool a `role:"tool"` result answers — Gemini links its `functionResponse`
    /// by function name, not by id, so the result must carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Multimodal content blocks (ADR 0030 phase 9). Additive + optional: when `None` the
    /// message is plain `content` text and serializes byte-identically to before. When set,
    /// the adapters fan it out to each provider's content-array wire form. `content` stays
    /// the text fallback (and the text part for providers that take a leading text block).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
}

impl LlmMessage {
    /// A plain text message (the common case): role + content, no tool fields.
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        LlmMessage {
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            content_blocks: None,
        }
    }

    /// A multimodal message (ADR 0030): role + content blocks. `content` keeps a plain-text
    /// digest of the text blocks (the fallback for any text-only consumer / redaction).
    pub fn with_blocks(role: impl Into<String>, blocks: Vec<ContentBlock>) -> Self {
        let content = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        LlmMessage {
            role: role.into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            content_blocks: Some(blocks),
        }
    }
}

/// One multimodal content block (ADR 0030 phase 9). A tagged enum on the wire
/// (`{ "type": "text", "text": … }` / `{ "type": "image", "source": … }` / …), mirroring the
/// provider content-array convention and the TS `LLMContentBlock` union.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContentBlock {
    Text { text: String },
    Image { source: MediaSource },
    Audio { source: MediaSource },
    File { source: MediaSource },
}

/// How a non-text block carries its payload (ADR 0030 D1/D5). **Default = `Artifact`**
/// (a small, stable pointer resolved to bytes just-in-time at the gateway boundary, so
/// checkpoints/events stay small + replay-stable). `Url` must be stable / content-addressed
/// (a volatile signed URL would break replay). `Base64` is the inline escape hatch — keep it
/// size-capped (the gateway/caller spills large media to the artifact store).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MediaSource {
    Artifact {
        artifact_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
        media_type: String,
    },
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    Base64 {
        media_type: String,
        data: String,
    },
}

impl MediaSource {
    /// The declared media type (`image/png`, `audio/mpeg`, …), if known.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            MediaSource::Artifact { media_type, .. } => Some(media_type),
            MediaSource::Base64 { media_type, .. } => Some(media_type),
            MediaSource::Url { media_type, .. } => media_type.as_deref(),
        }
    }

    /// `(media_type, base64_data)` when the bytes are inline. `None` for `Url` and for an
    /// unresolved `Artifact` (the gateway resolves artifacts to `Base64`/`Url` before the
    /// adapter runs — see ADR 0030 9c).
    pub fn as_base64(&self) -> Option<(&str, &str)> {
        match self {
            MediaSource::Base64 { media_type, data } => Some((media_type, data)),
            _ => None,
        }
    }

    /// A value usable as a provider URL field: a `data:<mime>;base64,<data>` URI for inline
    /// bytes, or the `Url` verbatim. `None` for an unresolved `Artifact`.
    pub fn as_url_or_data_uri(&self) -> Option<String> {
        match self {
            MediaSource::Base64 { media_type, data } => {
                Some(format!("data:{media_type};base64,{data}"))
            }
            MediaSource::Url { url, .. } => Some(url.clone()),
            MediaSource::Artifact { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

/// Provider-neutral request to constrain generation to a JSON shape (ADR 0029, phase 8).
/// One field on `LlmRequest`; each adapter fans it out to its own wire form
/// (OpenAI `response_format`, Anthropic forced tool, Gemini `responseSchema`). The
/// `schema` is a free-form JSON Schema `Value`, reusing the `LlmToolDef::input_schema`
/// convention rather than inventing a typed schema model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ResponseFormat {
    /// Constrain the response to JSON matching `schema`. `strict` requests the provider's
    /// strict-decoding mode where it exists (OpenAI `strict:true`); providers that lack it
    /// ignore the flag and rely on the in-engine validation floor (ADR 0029 D2/D3).
    JsonSchema {
        name: String,
        schema: Value,
        strict: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequest {
    pub provider: LlmProvider,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<LlmToolDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Constrain generation to a JSON shape (ADR 0029). Additive + optional: omitted
    /// requests behave exactly as before. Set by `StructuredOutputMiddleware`; each
    /// adapter translates it to its provider wire form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// The run (or subgraph child run, `{parent_run_id}:{node_id}` — same deterministic
    /// convention as `subgraph_run_id`) that made this call (ADR 0043). Additive + optional:
    /// `None` for any request built before this field existed. Set by the agent loop from its
    /// own `state.run_id`; recorded verbatim onto `RecordedCall` (it's simply part of this
    /// struct) so a recursive replay's existing `request == request` match also discriminates
    /// by run, without any separate journal-partitioning logic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Web search done BY THE MODEL PROVIDER for this call (ailu-core#284): Mistral's
    /// `web_search` connector (Conversations API) or Anthropic's `web_search` server tool.
    /// Additive + optional: `None` leaves the request byte-identical to before. A provider
    /// without web search, or a request that also carries client `tools`, is refused with
    /// [`crate::LlmError::WebSearchUnsupported`] — never a silent call without the web.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
}

/// How the provider may search the web for one call (ailu-core#284).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// Upper bound on searches for the call. Anthropic enforces it (`max_uses`); Mistral has
    /// no per-call bound, so there it is reported, not enforced.
    pub max_uses: u32,
    /// Only these domains (Anthropic `allowed_domains`). Not with `blocked_domains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    /// Never these domains (Anthropic `blocked_domains`). Not with `allowed_domains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,
}

/// One web page the provider consulted or cited, normalized across providers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSource {
    pub url: String,
    pub title: String,
    /// The passage the answer quotes (Anthropic `cited_text`, ≤ 150 chars), when cited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_text: Option<String>,
    /// When the page was last updated, as the provider reports it (Anthropic `page_age`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,
    /// `true` when the answer cites the page; `false` when it was only consulted.
    pub cited: bool,
}

/// What a call's web search produced (ailu-core#284). Part of the recorded response, so a
/// replay returns the same sources without touching the network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchOutcome {
    /// Deduplicated by URL, in order of first appearance.
    pub sources: Vec<WebSource>,
    /// Searches the provider ran (billed per search by Anthropic).
    pub requests: u32,
    /// The queries the model wrote, when the provider reports them — they cannot be seen
    /// before they leave, so they are shown after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    /// The provider's error code when a search failed (e.g. `max_uses_exceeded`); the call
    /// itself still returns an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WebSearchOutcome {
    /// Add a source, or merge it into the one already listed for that URL: a later citation
    /// marks it cited and supplies the quoted passage; the page age is kept from whichever
    /// block reported it. Order of first appearance is preserved.
    pub fn add_source(&mut self, source: WebSource) {
        match self.sources.iter_mut().find(|s| s.url == source.url) {
            Some(existing) => {
                existing.cited |= source.cited;
                if existing.cited_text.is_none() {
                    existing.cited_text = source.cited_text;
                }
                if existing.page_age.is_none() {
                    existing.page_age = source.page_age;
                }
                if existing.title.is_empty() {
                    existing.title = source.title;
                }
            }
            None => self.sources.push(source),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponse {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub usage: LlmUsage,
    pub model: String,
    pub provider: LlmProvider,
    /// Multimodal output blocks (ADR 0030 phase 9). Additive + optional: populated where a
    /// chat API returns inline media (e.g. Gemini inline images); `None` for text-only
    /// responses, which serialize byte-identically to before. Media *generation* on
    /// OpenAI/Anthropic is a separate provider API (a named future seam), not this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
    /// Set when the request asked for web search (ailu-core#284); `None` otherwise, so a
    /// response without web search serializes byte-identically to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    pub delta: String,
    pub done: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_only_message_omits_content_blocks_on_the_wire() {
        // ADR 0030 back-compat: a `::text` message serializes byte-identically to before —
        // the new `contentBlocks` field is absent when None.
        let value = serde_json::to_value(LlmMessage::text("user", "hi")).unwrap();
        assert_eq!(value, json!({ "role": "user", "content": "hi" }));
        assert!(value.get("contentBlocks").is_none());
    }

    #[test]
    fn with_blocks_carries_blocks_and_a_text_digest() {
        let msg = LlmMessage::with_blocks(
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
        );
        // `content` keeps a text digest (fallback for text-only consumers / redaction).
        assert_eq!(msg.content, "what is this?");
        assert_eq!(msg.content_blocks.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn content_block_and_media_source_serde_tags() {
        let block = ContentBlock::Image {
            source: MediaSource::Artifact {
                artifact_id: "a1".to_owned(),
                version: Some(3),
                media_type: "image/jpeg".to_owned(),
            },
        };
        let value = serde_json::to_value(&block).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "image",
                "source": { "kind": "artifact", "artifactId": "a1", "version": 3, "mediaType": "image/jpeg" }
            })
        );
        // Round-trips.
        let back: ContentBlock = serde_json::from_value(value).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn url_source_omits_optional_media_type() {
        let value = serde_json::to_value(MediaSource::Url {
            url: "https://cdn/x.png".to_owned(),
            media_type: None,
        })
        .unwrap();
        assert_eq!(value, json!({ "kind": "url", "url": "https://cdn/x.png" }));
    }

    #[test]
    fn text_only_response_omits_content_blocks() {
        let resp = LlmResponse {
            web_search: None,
            content: "ok".to_owned(),
            tool_calls: None,
            stop_reason: None,
            usage: LlmUsage::default(),
            model: "m".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        };
        let value = serde_json::to_value(&resp).unwrap();
        assert!(value.get("contentBlocks").is_none());
    }
}
