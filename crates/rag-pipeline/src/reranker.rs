//! Reranker — the `Reranker` seam plus the LLM-backed implementation.
//!
//! Ports `reranker/reranker.ts` (the `Reranker` interface) and
//! `reranker/llm-reranker.ts`. The deterministic glue — building the scoring
//! prompt, parsing the model's reply, falling back to the prior score on a
//! non-numeric reply, and sorting/truncating — is faithful to the TS; the model
//! call itself routes through the `ailu-llm-gateway` seam (the only crate
//! allowed to reach a provider).

use std::sync::Arc;

use ailu_llm_gateway::{LlmGateway, LlmMessage, LlmProvider, LlmRequest};
use async_trait::async_trait;

use crate::error::RagError;
use crate::types::RetrievalResult;

/// The reranker seam — re-score and reorder retrieval results for a query.
///
/// Mirrors the TS `Reranker` interface
/// (`rerank(query, results, topK): Promise<RetrievalResult[]>`).
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Re-score `results` against `query` and return the top `top_k`.
    async fn rerank(
        &self,
        query: &str,
        results: Vec<RetrievalResult>,
        top_k: usize,
    ) -> Result<Vec<RetrievalResult>, RagError>;
}

/// LLM-backed reranker: ask the gateway to score each chunk's relevance to the
/// query, parse the numeric reply, and reorder.
///
/// Faithful port of the TS `LLMReranker`: for each result it sends a
/// `provider: "openai", model: "mock-reranker"` request whose user message is
/// `"Score relevance from 0 to 1.\nQuery: {query}\nText: {content}"`, parses the
/// reply with JavaScript `Number.parseFloat` semantics, falls back to the prior
/// score when the reply is not a finite number, then sorts descending and takes
/// the top `top_k`.
pub struct LlmReranker {
    gateway: Arc<dyn LlmGateway>,
}

impl LlmReranker {
    /// Create a reranker over an LLM gateway.
    pub fn new(gateway: Arc<dyn LlmGateway>) -> Self {
        Self { gateway }
    }
}

/// Parse a leading floating-point number from `s`, mirroring JavaScript
/// `Number.parseFloat`: skip leading whitespace, consume an optional sign, the
/// longest valid numeric prefix (digits, a single decimal point, and an optional
/// exponent), and ignore any trailing characters. Returns `None` when no number
/// is present (the JS `NaN` case).
fn parse_leading_f64(s: &str) -> Option<f64> {
    let bytes = s.trim_start().as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;

    // Optional leading sign.
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => {
                seen_digit = true;
                end += 1;
            }
            b'.' if !seen_dot && !seen_exp => {
                seen_dot = true;
                end += 1;
            }
            b'e' | b'E' if seen_digit && !seen_exp => {
                seen_exp = true;
                end += 1;
                // Optional sign immediately after the exponent marker.
                if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                    end += 1;
                }
            }
            _ => break,
        }
    }

    if !seen_digit {
        return None;
    }
    let candidate = &s.trim_start()[..end];
    candidate.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[async_trait]
impl Reranker for LlmReranker {
    async fn rerank(
        &self,
        query: &str,
        results: Vec<RetrievalResult>,
        top_k: usize,
    ) -> Result<Vec<RetrievalResult>, RagError> {
        // `if (results.length === 0) return [];`
        if results.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored: Vec<RetrievalResult> = Vec::with_capacity(results.len());
        for result in results {
            let request = LlmRequest {
                provider: LlmProvider::Openai,
                model: "mock-reranker".to_string(),
                messages: vec![LlmMessage::text(
                    "user",
                    format!(
                        "Score relevance from 0 to 1.\nQuery: {}\nText: {}",
                        query,
                        result.chunk.content()
                    ),
                )],
                system: None,
                tools: None,
                max_tokens: None,
                temperature: None,
                response_format: None,
                run_id: None,
            };
            let response = self
                .gateway
                .complete(request)
                .await
                .map_err(|e| RagError::Reranker(e.to_string()))?;
            // `Number.isFinite(parsed) ? parsed : result.score`
            let score = parse_leading_f64(&response.content).unwrap_or(result.score);
            scored.push(RetrievalResult {
                chunk: result.chunk,
                score,
            });
        }

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Chunk, Document};
    use ailu_llm_gateway::{LlmError, LlmResponse, LlmUsage};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A gateway that returns a canned reply keyed by the chunk text embedded in
    /// the prompt's last line.
    struct ScriptedGateway {
        replies: Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl LlmGateway for ScriptedGateway {
        async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
            let prompt = &request.messages[0].content;
            let replies = self.replies.lock().unwrap();
            let content = replies
                .iter()
                .find(|(text, _)| prompt.contains(text.as_str()))
                .map(|(_, reply)| reply.clone())
                .unwrap_or_default();
            Ok(LlmResponse {
                content,
                tool_calls: None,
                stop_reason: Some("end_turn".to_string()),
                usage: LlmUsage::default(),
                model: request.model,
                provider: request.provider,
                content_blocks: None,
            })
        }
    }

    fn result(id: &str, content: &str, score: f64) -> RetrievalResult {
        RetrievalResult {
            chunk: Chunk {
                document: Document::new(id, content),
                source_id: "d1".to_string(),
                chunk_index: 0,
            },
            score,
        }
    }

    #[test]
    fn parse_leading_f64_matches_js() {
        assert_eq!(parse_leading_f64("0.8"), Some(0.8));
        assert_eq!(parse_leading_f64("  0.42 is the score"), Some(0.42));
        assert_eq!(parse_leading_f64("not a number"), None);
        assert_eq!(parse_leading_f64(""), None);
        assert_eq!(parse_leading_f64("1e2x"), Some(100.0));
    }

    #[tokio::test]
    async fn reranks_by_model_score_with_fallback() {
        let mut replies = HashMap::new();
        replies.insert("low relevance".to_string(), "0.1".to_string());
        replies.insert("high relevance".to_string(), "0.9".to_string());
        // "no score" -> non-numeric reply -> falls back to its prior score (0.5).
        replies.insert("no score".to_string(), "n/a".to_string());
        let gateway = Arc::new(ScriptedGateway {
            replies: Mutex::new(replies),
        });
        let reranker = LlmReranker::new(gateway);

        let results = vec![
            result("c1", "low relevance", 0.0),
            result("c2", "high relevance", 0.0),
            result("c3", "no score", 0.5),
        ];
        let ranked = reranker.rerank("q", results, 3).await.unwrap();
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].chunk.id(), "c2"); // 0.9
        assert_eq!(ranked[1].chunk.id(), "c3"); // 0.5 (fallback)
        assert_eq!(ranked[2].chunk.id(), "c1"); // 0.1
    }

    #[tokio::test]
    async fn empty_results_short_circuit() {
        let gateway = Arc::new(ScriptedGateway {
            replies: Mutex::new(HashMap::new()),
        });
        let reranker = LlmReranker::new(gateway);
        let ranked = reranker.rerank("q", Vec::new(), 5).await.unwrap();
        assert!(ranked.is_empty());
    }
}
