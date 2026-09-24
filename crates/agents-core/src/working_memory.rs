//! Working memory — the Rust port of `@ailu-ai/agents-core`'s `WorkingMemory` +
//! `compressShortTerm` (`working-memory.ts`).
//!
//! The TS keeps a short-term buffer of `Message`s and compresses it with an LLM call
//! when it exceeds a token budget. `compressShortTerm` is the whole behaviour:
//!
//! 1. estimate tokens per message as `max(1, ceil(content.length / 4))` and sum;
//! 2. if the total is `<= maxTokens`, return the buffer unchanged (no LLM call);
//! 3. otherwise keep the most recent `keepCount = max(1, floor(len / 2))` messages,
//!    summarize the rest in one LLM call, and rebuild the buffer as
//!    `[summary, ...kept]` where `summary` is a `system` message holding the
//!    completion;
//! 4. if the rebuilt buffer is *still* over budget, truncate to its last
//!    `max(1, floor(maxTokens / 4))` messages.
//!
//! Seam note: the Rust `graph-core` has no `Message` type yet, so this module
//! defines the smallest [`Message`] it needs (`id`/`role`/`content`/`created_at`) to
//! mirror the TS shape. The TS summary id is `summary:${Date.now()}`; to stay
//! deterministic (no wall-clock dependency) this port uses `summary:<n>` with a
//! per-`WorkingMemory` compression counter — the only intentional divergence.

use std::sync::Arc;

use ailu_llm_gateway::{LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest};
use serde::{Deserialize, Serialize};

/// Model used for the compression call. Matches the TS
/// `model: "working-memory-compressor"`.
pub const COMPRESSOR_MODEL: &str = "working-memory-compressor";

/// One short-term message. Smallest faithful port of the TS `Message` union; roles
/// are free-form strings (`"human" | "ai" | "tool" | "system"` in TS, `"system"`
/// for the synthesized summary).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl Message {
    pub fn new(id: impl Into<String>, role: impl Into<String>, content: impl Into<String>) -> Self {
        Message {
            id: id.into(),
            role: role.into(),
            content: content.into(),
            created_at: String::new(),
        }
    }
}

/// Token estimate for one message: `max(1, ceil(content.len / 4))`, byte-for-byte
/// the TS `defaultCountFn` (length is character count in TS; this port uses the
/// `char` count to match for non-ASCII content too).
fn count_tokens(message: &Message) -> usize {
    let chars = message.content.chars().count();
    // ceil(chars / 4), floored at 1.
    chars.div_ceil(4).max(1)
}

fn total_tokens(messages: &[Message]) -> usize {
    messages.iter().map(count_tokens).sum()
}

/// A short-term buffer of messages plus a compression counter (used to mint
/// deterministic summary ids). The TS `WorkingMemory.longTerm` store is out of scope
/// for this slice and intentionally not modelled here.
#[derive(Clone, Debug, Default)]
pub struct WorkingMemory {
    pub short_term: Vec<Message>,
    compressions: usize,
}

impl WorkingMemory {
    pub fn new() -> Self {
        WorkingMemory::default()
    }

    pub fn with_messages(messages: Vec<Message>) -> Self {
        WorkingMemory {
            short_term: messages,
            compressions: 0,
        }
    }

    /// Append a message to the short-term buffer.
    pub fn push(&mut self, message: Message) {
        self.short_term.push(message);
    }

    /// Compress the short-term buffer in place if (and only if) it is over
    /// `max_tokens`, mirroring `compressShortTerm`. Returns `true` if a compression
    /// (and thus an LLM call) happened, `false` if the buffer was already within
    /// budget and left untouched.
    pub async fn compress_short_term(
        &mut self,
        llm: &Arc<dyn LlmGateway>,
        max_tokens: usize,
    ) -> Result<bool, LlmError> {
        let compressed = compress_short_term(
            &self.short_term,
            llm,
            max_tokens,
            self.compressions,
            self.provider(),
        )
        .await?;
        match compressed {
            Some(messages) => {
                self.short_term = messages;
                self.compressions += 1;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn provider(&self) -> LlmProvider {
        LlmProvider::Anthropic
    }
}

/// Pure compression core. Returns `None` when the buffer is within budget (no LLM
/// call), or `Some(rebuilt_buffer)` after summarizing. `summary_seq` is the suffix
/// for the synthesized summary message id (`summary:<seq>`).
async fn compress_short_term(
    messages: &[Message],
    llm: &Arc<dyn LlmGateway>,
    max_tokens: usize,
    summary_seq: usize,
    provider: LlmProvider,
) -> Result<Option<Vec<Message>>, LlmError> {
    if total_tokens(messages) <= max_tokens {
        return Ok(None);
    }

    let len = messages.len();
    // keepCount = max(1, floor(len / 2)); split off the oldest to summarize.
    let keep_count = (len / 2).max(1);
    let split = len.saturating_sub(keep_count);
    let to_summarize = &messages[..split];
    let to_keep = &messages[split..];

    // TS: JSON.stringify(toSummarize.map(({ role, content }) => …)).
    let payload = serde_json::to_string(
        &to_summarize
            .iter()
            .map(|message| serde_json::json!({ "role": message.role, "content": message.content }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_owned());

    let response = llm
        .complete(LlmRequest {
            provider,
            model: COMPRESSOR_MODEL.to_owned(),
            messages: vec![LlmMessage::text(
                "user",
                format!("Summarize briefly:\n{payload}"),
            )],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        })
        .await?;

    let summary = Message::new(format!("summary:{summary_seq}"), "system", response.content);

    let mut compressed = Vec::with_capacity(1 + to_keep.len());
    compressed.push(summary);
    compressed.extend(to_keep.iter().cloned());

    // If still over budget, keep only the last max(1, floor(maxTokens / 4)) messages.
    if total_tokens(&compressed) > max_tokens {
        let tail = (max_tokens / 4).max(1);
        let start = compressed.len().saturating_sub(tail);
        compressed = compressed.split_off(start);
    }

    Ok(Some(compressed))
}

#[cfg(test)]
mod tests {
    use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmUsage, MockAdapter};

    use super::*;

    fn text(content: &str) -> LlmResponse {
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

    fn gateway_with(responses: Vec<LlmResponse>) -> Arc<dyn LlmGateway> {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            responses,
        )));
        Arc::new(gateway)
    }

    fn msg(id: &str, content: &str) -> Message {
        Message::new(id, "human", content)
    }

    #[tokio::test]
    async fn does_not_compress_when_within_budget() {
        let gateway = gateway_with(vec![text("should not be called")]);
        // Two short messages: each "hi" is 2 chars → ceil(2/4)=1, floored to 1 → 2 total.
        let mut memory = WorkingMemory::with_messages(vec![msg("m1", "hi"), msg("m2", "yo")]);
        let before = memory.short_term.clone();

        let compressed = memory.compress_short_term(&gateway, 10).await.unwrap();

        assert!(!compressed);
        assert_eq!(memory.short_term, before);
    }

    #[tokio::test]
    async fn compresses_when_over_budget_into_summary_plus_kept_tail() {
        let gateway = gateway_with(vec![text("SUMMARY")]);
        // Four messages of 8 chars each → 2 tokens each → 8 total, over a budget of 4.
        let mut memory = WorkingMemory::with_messages(vec![
            msg("m1", "aaaaaaaa"),
            msg("m2", "bbbbbbbb"),
            msg("m3", "cccccccc"),
            msg("m4", "dddddddd"),
        ]);

        let compressed = memory.compress_short_term(&gateway, 4).await.unwrap();

        assert!(compressed);
        // keepCount = max(1, floor(4/2)) = 2 → keep m3, m4; summarize m1, m2.
        // Rebuilt = [summary, m3, m4] = 2 + 2 + 2 = 6 tokens, still > 4, so truncate
        // to the last max(1, floor(4/4)) = 1 message → just [m4].
        assert_eq!(memory.short_term.len(), 1);
        assert_eq!(memory.short_term[0].id, "m4");
    }

    #[tokio::test]
    async fn compressed_buffer_keeps_summary_when_within_budget_after_summarizing() {
        let gateway = gateway_with(vec![text("S")]);
        // Three messages of 8 chars (2 tokens each) → 6 total, over budget 5.
        let mut memory = WorkingMemory::with_messages(vec![
            msg("m1", "aaaaaaaa"),
            msg("m2", "bbbbbbbb"),
            msg("m3", "cccccccc"),
        ]);

        let compressed = memory.compress_short_term(&gateway, 5).await.unwrap();

        assert!(compressed);
        // keepCount = max(1, floor(3/2)) = 1 → keep m3; summarize m1, m2.
        // summary "S" = ceil(1/4)=1 token; rebuilt = [summary, m3] = 1 + 2 = 3 <= 5,
        // so no truncation: the summary survives at the head.
        assert_eq!(memory.short_term.len(), 2);
        assert_eq!(memory.short_term[0].id, "summary:0");
        assert_eq!(memory.short_term[0].role, "system");
        assert_eq!(memory.short_term[0].content, "S");
        assert_eq!(memory.short_term[1].id, "m3");
    }

    #[tokio::test]
    async fn compression_counter_advances_the_summary_id() {
        // "S" summaries cost 1 token; 8-char messages cost 2 tokens. Budget 5 keeps
        // the summary at the head after each compression so we can read its id.
        let gateway = gateway_with(vec![text("S"), text("S")]);
        let m = |id: &str| msg(id, "aaaaaaaa");
        let mut memory = WorkingMemory::with_messages(vec![m("a"), m("b"), m("c")]);

        // First compression (6 > 5): keep last (c), summarize a,b → [summary:0, c] = 3.
        assert!(memory.compress_short_term(&gateway, 5).await.unwrap());
        assert_eq!(memory.short_term[0].id, "summary:0");

        // Now [summary:0, c, d, e] = 1+2+2+2 = 7 > 5: keep last 2 (d,e), summarize the
        // first two → [summary:1, d, e] = 5 <= 5, so the new summary survives at head.
        memory.push(m("d"));
        memory.push(m("e"));
        assert!(memory.compress_short_term(&gateway, 5).await.unwrap());
        assert_eq!(memory.short_term[0].id, "summary:1");
    }

    #[test]
    fn message_serializes_camel_case() {
        let message = Message::new("summary:0", "system", "hello");
        let wire = serde_json::to_string(&message).expect("serializes");
        assert!(wire.contains("\"createdAt\""));
        let back: Message = serde_json::from_str(&wire).expect("round-trips");
        assert_eq!(back, message);
    }
}
