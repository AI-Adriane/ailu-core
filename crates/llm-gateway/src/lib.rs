//! Ailu llm-gateway (Rust).
//!
//! The only crate allowed to talk to LLM providers — the Rust port of
//! `@ailu/llm-gateway`. Requests route through a gateway to a provider adapter
//! (`complete()`) and, opt-in, stream per-token deltas (`stream()`, ADR 0033) with the
//! Anthropic / OpenAI-compatible / Gemini adapters all backing real provider SSE.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod compressor;
pub mod cross_encoder;
pub mod error;
pub mod gateway;
pub mod gemini;
pub mod http;
pub mod media_resolver;
pub mod mock;
pub mod model_policy;
pub mod openai_compatible;
pub mod redactor;
pub mod replay_journal;
pub mod secrets_redactor;
pub mod sse;
pub mod types;

pub use anthropic::{
    build_request_body, AnthropicAdapter, AnthropicCreateParams, AnthropicMessage, AnthropicPort,
    AnthropicRawResponse, AnthropicRole, AnthropicUsage, HttpAnthropicPort, SystemBlock, ToolParam,
    DEFAULT_MAX_TOKENS, DEFAULT_MODEL,
};
pub use compressor::{
    CompressingGateway, HttpPromptCompressor, NoopPromptCompressor, PromptCompressor,
};
pub use cross_encoder::{
    CrossEncoderReranker, HttpRerankTransport, RerankDoc, RerankResult, RerankTransport,
};
pub use error::LlmError;
pub use gateway::{DefaultLlmGateway, LlmGateway, LlmProviderAdapter};
pub use gemini::{
    build_request_body as build_gemini_request_body, GeminiAdapter, GeminiCandidate, GeminiContent,
    GeminiFunctionCall, GeminiInlineData, GeminiPart, GeminiPort, GeminiRawResponse,
    GeminiUsageMetadata, HttpGeminiPort, DEFAULT_GEMINI_MODEL,
};
pub use media_resolver::{resolve_request_media, MediaResolver, MAX_INLINE_MEDIA_BASE64_LEN};
pub use mock::MockAdapter;
pub use model_policy::{ModelChoice, ModelPolicy, ModelTier};
pub use openai_compatible::{
    build_request_body as build_openai_request_body, HttpPort, OpenAiChatResponse,
    OpenAiCompatibleAdapter, OpenAiCompatiblePort, RawChoice, RawFunctionCall, RawMessage,
    RawToolCall, RawUsage, HUGGINGFACE_BASE_URL, HUGGINGFACE_DEFAULT_MODEL, LMSTUDIO_BASE_URL,
    LMSTUDIO_DEFAULT_MODEL, MINIMAX_BASE_URL, MINIMAX_DEFAULT_MODEL, MISTRAL_BASE_URL,
    MISTRAL_DEFAULT_MODEL, OLLAMA_BASE_URL, OLLAMA_DEFAULT_MODEL, OPENAI_BASE_URL,
    OPENAI_DEFAULT_MODEL, OPENROUTER_BASE_URL, OPENROUTER_DEFAULT_MODEL,
};
pub use redactor::{HttpPiiRedactor, NoopPiiRedactor, PiiRedactor, RedactingGateway};
pub use replay_journal::{LlmJournal, RecordedCall, RecordingGateway, ReplayGateway};
pub use secrets_redactor::{scrub_secrets, RegexSecretsRedactor, SecretPolicy};
pub use types::{
    ContentBlock, LlmMessage, LlmProvider, LlmRequest, LlmResponse, LlmStreamChunk, LlmToolCall,
    LlmToolDef, LlmUsage, MediaSource, ResponseFormat,
};
