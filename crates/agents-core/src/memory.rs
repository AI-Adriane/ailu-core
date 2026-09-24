//! Governed long-term memory middleware (ADR 0026 phase 11).
//!
//! Wires the [`ailu_memory`] seam into the agent loop via the proven `before_run` /
//! `after_run` hooks: **recall before** (embed the seed, vector-recall from the namespace, inject
//! the hits into the seed) and **persist after** (store the run's reasoning as a recallable item,
//! tagged with [`MemoryProvenance`]). Installed as a **governed** middleware constructed with its
//! `namespace` + `principal` sealed (the bridge supplies them; user data never does), so recall is
//! tenant-scoped by construction. Fail-open: a recall/persist error never sinks an otherwise-good
//! run. Determinism: recall only mutates the SEED conversation (no runtime state change), and the
//! seam's ordering is deterministic (score-desc + insertion-order tie-break).
//!
//! Scope: vector recall + provenance-tagged persist (heuristic — NO LLM claim-writing). The
//! entity-graph seam (`put_entity`/`put_edge`/`neighbors`) is available for callers / the
//! control plane; governed LLM entity extraction is deferred (ADR 0026 §2 / control-plane).

use std::sync::Arc;

use ailu_llm_gateway::{LlmError, LlmMessage};
use ailu_memory::{
    Embedder, MemoryItem, MemoryProvenance, MemoryStore, RecallMode, RetrievalPolicy,
};
use sha2::{Digest, Sha256};

use crate::middleware::{AgentMiddleware, Flow, RunCtx};
use crate::react::AgentResult;

/// Reserved input channel (ADR 0026 Phase B): durable memory items the CONTROL PLANE recalled from
/// the persistent store (e.g. Neo4j) before the run and seeded here, as a JSON array of strings.
/// When present, `before_run` injects these (cross-process durable recall) instead of the in-process
/// store recall — the OSS default still recalls from the in-process store when the channel is absent.
pub const MEMORY_RECALL_CHANNEL: &str = "__memoryRecall";

/// Read `__memoryRecall` as a list of recalled memory texts (control-plane-seeded). Tolerant: a
/// missing/malformed channel yields an empty list (fail-open — never sinks the run).
fn read_recall_channel(
    channels: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Vec<String> {
    channels
        .get(MEMORY_RECALL_CHANNEL)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Recall-before / persist-after long-term memory over the [`MemoryStore`] seam.
pub struct MemoryMiddleware {
    store: Arc<dyn MemoryStore>,
    embedder: Arc<dyn Embedder>,
    /// Tenant-scoped namespace, sealed at construction (the bridge supplies it).
    namespace: String,
    /// Who the writes are attributed to (an agent id / human), sealed at construction.
    principal: Option<String>,
    policy: RetrievalPolicy,
}

impl MemoryMiddleware {
    pub fn new(
        store: Arc<dyn MemoryStore>,
        embedder: Arc<dyn Embedder>,
        namespace: impl Into<String>,
        principal: Option<String>,
        policy: RetrievalPolicy,
    ) -> Self {
        Self {
            store,
            embedder,
            namespace: namespace.into(),
            principal,
            policy,
        }
    }

    async fn embed_one(&self, text: &str) -> Option<Vec<f64>> {
        self.embedder
            .embed(&[text.to_owned()])
            .await
            .ok()
            .and_then(|vectors| vectors.into_iter().next())
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for MemoryMiddleware {
    fn name(&self) -> &str {
        "memory"
    }

    async fn before_run(
        &self,
        conversation: &mut Vec<LlmMessage>,
        ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        // Vector recall runs for Vector | Both. Graph-only mode auto-recall needs entity linking
        // (deferred to control-plane extraction) — the seam's neighbors() is available meanwhile.
        if !matches!(self.policy.mode, RecallMode::Vector | RecallMode::Both) {
            return Ok(Flow::Continue);
        }
        // Phase B durable recall: if the control plane seeded `__memoryRecall` with items it recalled
        // from the persistent store (Neo4j) pre-run, prefer those (cross-process, durable). When the
        // channel is absent we fall back to the in-process store (the OSS default). Either way recall
        // only mutates the SEED — no runtime state change (determinism preserved).
        let recalled = {
            let durable = read_recall_channel(ctx.channels);
            if !durable.is_empty() {
                durable
            } else {
                let Some(seed_text) = conversation.first().map(|m| m.content.clone()) else {
                    return Ok(Flow::Continue);
                };
                // Fail-open throughout: a recall failure must not sink the run.
                let Some(query) = self.embed_one(&seed_text).await else {
                    return Ok(Flow::Continue);
                };
                match self
                    .store
                    .recall_by_vector(&self.namespace, &query, self.policy.top_k)
                    .await
                {
                    Ok(hits) if !hits.is_empty() => {
                        hits.into_iter().map(|hit| hit.text).collect::<Vec<_>>()
                    }
                    _ => return Ok(Flow::Continue),
                }
            }
        };
        let recalled = recalled
            .iter()
            .map(|text| format!("- {text}"))
            .collect::<Vec<_>>()
            .join("\n");
        let block = format!(
            "Relevant memory (recalled from '{}'):\n{recalled}",
            self.namespace
        );
        // Prepend into the seed message — one message, no role-alternation break.
        if let Some(seed) = conversation.first_mut() {
            seed.content = format!("{block}\n\n{}", seed.content);
        }
        Ok(Flow::Continue)
    }

    async fn after_run(&self, result: &mut AgentResult, _ctx: &RunCtx<'_>) -> Result<(), LlmError> {
        // Heuristic persist: store the run's reasoning as a recallable, attributable memory.
        // No LLM claim-writing (governed entity extraction is control-plane, ADR 0026 §2).
        let text = result.reasoning.clone();
        if text.trim().is_empty() {
            return Ok(());
        }
        let embedding = self.embed_one(&text).await;
        // Deterministic, dedup-friendly key (the engine has no clock; the control plane stamps time).
        let key = format!("{:x}", Sha256::digest(text.as_bytes()));
        let item = MemoryItem {
            namespace: self.namespace.clone(),
            key,
            text,
            embedding,
            provenance: MemoryProvenance {
                principal: self.principal.clone(),
                status: Some("asserted".to_owned()),
                ..Default::default()
            },
        };
        // Fail-open: a persist error never fails the run.
        let _ = self.store.put_item(item).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_memory::{InMemoryMemoryStore, MockEmbedder};
    use std::collections::{BTreeMap, HashSet};

    fn ctx<'a>(
        approved: &'a HashSet<String>,
        channels: &'a BTreeMap<String, serde_json::Value>,
    ) -> RunCtx<'a> {
        RunCtx {
            iteration: 0,
            approved_tool_names: approved,
            channels,
            run_id: None,
        }
    }

    fn result_with(reasoning: &str) -> AgentResult {
        AgentResult {
            reasoning: reasoning.to_owned(),
            approval_requests: vec![],
            requires_human_review: false,
            todos: None,
            usage: None,
            structured_output: None,
            memory_writes: None,
        }
    }

    #[tokio::test]
    async fn persists_then_recalls_into_the_seed() {
        let store = Arc::new(InMemoryMemoryStore::new());
        let embedder = Arc::new(MockEmbedder);
        let mw = MemoryMiddleware::new(
            store.clone(),
            embedder,
            "ns",
            Some("agent:a".to_owned()),
            RetrievalPolicy::default(),
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();

        // after_run persists the reasoning (attributed).
        let mut result = result_with("the capital of France is Paris");
        mw.after_run(&mut result, &ctx(&approved, &channels))
            .await
            .unwrap();

        // before_run recalls it into the seed of a fresh run with a related query.
        let mut conversation = vec![LlmMessage::text(
            "user",
            "Input: what is the capital of France?",
        )];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert!(conversation[0].content.contains("Relevant memory"));
        assert!(conversation[0].content.contains("Paris"));
        // The original seed text is preserved after the recalled block.
        assert!(conversation[0]
            .content
            .contains("Input: what is the capital of France?"));
    }

    #[tokio::test]
    async fn durable_recall_channel_injects_without_touching_the_store() {
        // Empty in-process store: only the control-plane-seeded `__memoryRecall` channel can supply
        // recalled items (Phase B — durable Neo4j recall happens in the control plane, not here).
        let store = Arc::new(InMemoryMemoryStore::new());
        let mw = MemoryMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "tenant:t1:agent:a",
            None,
            RetrievalPolicy::default(),
        );
        let approved = HashSet::new();
        let mut channels = BTreeMap::new();
        channels.insert(
            MEMORY_RECALL_CHANNEL.to_owned(),
            serde_json::json!(["the user prefers metric units", "deploy target is Scaleway"]),
        );
        let mut conversation = vec![LlmMessage::text("user", "Input: what units?")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert!(conversation[0].content.contains("Relevant memory"));
        assert!(conversation[0].content.contains("prefers metric units"));
        assert!(conversation[0].content.contains("Scaleway"));
        // Original seed preserved after the recalled block.
        assert!(conversation[0].content.contains("Input: what units?"));
    }

    #[tokio::test]
    async fn empty_store_leaves_the_seed_unchanged() {
        let store = Arc::new(InMemoryMemoryStore::new());
        let mw = MemoryMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "ns",
            None,
            RetrievalPolicy::default(),
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: hi")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert_eq!(conversation[0].content, "Input: hi");
    }

    #[tokio::test]
    async fn graph_only_mode_skips_vector_recall() {
        let store = Arc::new(InMemoryMemoryStore::new());
        store
            .put_item(MemoryItem {
                namespace: "ns".to_owned(),
                key: "k".to_owned(),
                text: "something".to_owned(),
                embedding: Some(vec![1.0, 0.0]),
                provenance: MemoryProvenance::default(),
            })
            .await
            .unwrap();
        let mw = MemoryMiddleware::new(
            store,
            Arc::new(MockEmbedder),
            "ns",
            None,
            RetrievalPolicy {
                top_k: 5,
                mode: RecallMode::Graph,
            },
        );
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let mut conversation = vec![LlmMessage::text("user", "Input: hi")];
        mw.before_run(&mut conversation, &ctx(&approved, &channels))
            .await
            .unwrap();
        assert_eq!(conversation[0].content, "Input: hi"); // graph-only → no vector inject
    }
}
