//! Cross-encoder reranking (ADR 0060 E1, approach C) — re-score retrieval candidates against the query
//! with a real cross-encoder (`BAAI/bge-reranker-v2-m3`) served by a self-hostable, EU-sovereign rerank
//! service (HuggingFace TEI). The gateway is the only crate allowed to reach an external service, so the
//! HTTP call lives here behind a transport seam (offline-testable). **Graceful fallback**: with no
//! `ADRIANE_RERANK_ENDPOINT` configured, `rerank` is an identity passthrough — the pipeline degrades to
//! the upstream RRF ranking with zero cross-encoder and zero external dependency.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;

/// A candidate document to re-score against the query.
#[derive(Clone, Debug)]
pub struct RerankDoc {
    pub id: String,
    pub content: String,
}

/// A re-scored candidate (higher `score` = more relevant). The reranker returns these sorted desc.
#[derive(Clone, Debug, PartialEq)]
pub struct RerankResult {
    pub id: String,
    pub score: f64,
}

/// Transport seam for the rerank HTTP call — injected so tests stay offline.
#[async_trait]
pub trait RerankTransport: Send + Sync {
    /// POST `query` + `texts` to `endpoint`; return one score per text, in the SAME order as `texts`.
    async fn score(
        &self,
        endpoint: &str,
        query: &str,
        texts: &[String],
    ) -> Result<Vec<f64>, LlmError>;
}

#[derive(Serialize)]
struct TeiRequest<'a> {
    query: &'a str,
    texts: &'a [String],
}

#[derive(Deserialize)]
struct TeiScore {
    index: usize,
    score: f64,
}

/// Real reqwest transport for a HuggingFace TEI `/rerank` endpoint: body `{ query, texts }`, response
/// `[{ index, score }, …]` (TEI may reorder by score, so scores are remapped back to input order).
pub struct HttpRerankTransport {
    client: reqwest::Client,
}

impl HttpRerankTransport {
    pub fn new() -> Self {
        Self {
            client: crate::http::http_client(),
        }
    }
}

impl Default for HttpRerankTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RerankTransport for HttpRerankTransport {
    async fn score(
        &self,
        endpoint: &str,
        query: &str,
        texts: &[String],
    ) -> Result<Vec<f64>, LlmError> {
        let response = self
            .client
            .post(endpoint)
            .json(&TeiRequest { query, texts })
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("rerank request failed: {e}")))?;
        if !response.status().is_success() {
            return Err(LlmError::Provider(format!(
                "rerank endpoint returned {}",
                response.status()
            )));
        }
        let scored: Vec<TeiScore> = response
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("rerank response parse failed: {e}")))?;
        // Remap to input order — TEI returns items sorted by score, keyed by the original index.
        let mut out = vec![0.0_f64; texts.len()];
        for item in scored {
            if item.index < out.len() {
                out[item.index] = item.score;
            }
        }
        Ok(out)
    }
}

/// Cross-encoder reranker over a self-hostable rerank service (ADR 0060 E1, approach C).
pub struct CrossEncoderReranker {
    endpoint: Option<String>,
    transport: Arc<dyn RerankTransport>,
}

impl CrossEncoderReranker {
    /// Construct with an explicit endpoint (or `None` to force the passthrough fallback).
    pub fn new(endpoint: Option<String>, transport: Arc<dyn RerankTransport>) -> Self {
        Self {
            endpoint: endpoint.filter(|e| !e.trim().is_empty()),
            transport,
        }
    }

    /// Read the endpoint from `ADRIANE_RERANK_ENDPOINT` (empty/unset → passthrough fallback).
    pub fn from_env(transport: Arc<dyn RerankTransport>) -> Self {
        Self::new(std::env::var("ADRIANE_RERANK_ENDPOINT").ok(), transport)
    }

    /// True when a cross-encoder endpoint is configured (else `rerank` is an identity passthrough).
    pub fn enabled(&self) -> bool {
        self.endpoint.is_some()
    }

    /// Re-score `docs` against `query` and return the top `top_k`, sorted by score desc (ties broken by
    /// id asc for determinism). With no endpoint configured, returns the first `top_k` docs in their
    /// given order (identity passthrough — the upstream ranking is preserved).
    pub async fn rerank(
        &self,
        query: &str,
        docs: Vec<RerankDoc>,
        top_k: usize,
    ) -> Result<Vec<RerankResult>, LlmError> {
        let Some(endpoint) = self.endpoint.as_deref() else {
            return Ok(docs
                .into_iter()
                .take(top_k)
                .enumerate()
                .map(|(i, doc)| RerankResult {
                    id: doc.id,
                    // Descending identity scores preserve the given order without a cross-encoder.
                    score: -(i as f64),
                })
                .collect());
        };
        let texts: Vec<String> = docs.iter().map(|doc| doc.content.clone()).collect();
        let scores = self.transport.score(endpoint, query, &texts).await?;
        let mut scored: Vec<RerankResult> = docs
            .into_iter()
            .zip(scores)
            .map(|(doc, score)| RerankResult { id: doc.id, score })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        scored.truncate(top_k);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransport {
        scores: Vec<f64>,
    }

    #[async_trait]
    impl RerankTransport for FakeTransport {
        async fn score(&self, _e: &str, _q: &str, texts: &[String]) -> Result<Vec<f64>, LlmError> {
            assert_eq!(texts.len(), self.scores.len());
            Ok(self.scores.clone())
        }
    }

    fn docs() -> Vec<RerankDoc> {
        vec![
            RerankDoc {
                id: "a".into(),
                content: "alpha".into(),
            },
            RerankDoc {
                id: "b".into(),
                content: "beta".into(),
            },
            RerankDoc {
                id: "c".into(),
                content: "gamma".into(),
            },
        ]
    }

    #[tokio::test]
    async fn reorders_by_cross_encoder_score_and_truncates() {
        let reranker = CrossEncoderReranker::new(
            Some("http://rerank".into()),
            Arc::new(FakeTransport {
                scores: vec![0.1, 0.9, 0.5],
            }),
        );
        assert!(reranker.enabled());
        let out = reranker.rerank("q", docs(), 2).await.unwrap();
        assert_eq!(
            out.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[tokio::test]
    async fn passthrough_preserves_order_when_no_endpoint() {
        let reranker = CrossEncoderReranker::new(None, Arc::new(FakeTransport { scores: vec![] }));
        assert!(!reranker.enabled());
        let out = reranker.rerank("q", docs(), 2).await.unwrap();
        assert_eq!(
            out.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[tokio::test]
    async fn blank_endpoint_falls_back_to_passthrough() {
        let reranker = CrossEncoderReranker::new(
            Some("  ".into()),
            Arc::new(FakeTransport { scores: vec![] }),
        );
        assert!(!reranker.enabled());
    }
}
