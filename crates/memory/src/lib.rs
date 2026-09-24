//! `ailu-memory` — unified, governed long-term memory (ADR 0026 phase 11).
//!
//! One home for the two recall modalities the thesis needs — **vector** (semantic) and
//! **graph** (entities + typed edges) — behind a single DB-free [`MemoryStore`] seam with an
//! in-memory default. Every write carries [`MemoryProvenance`] (who / what / when / from-which
//! source / confidence / status), so memory is attributable from day one. Retrieval is
//! deterministic: score-desc with an explicit insertion-order tie-break (reusing the
//! `rag-pipeline` cosine), so a governed run's reads are reproducible.
//!
//! Engine-OSS ships only the in-memory default ([`InMemoryMemoryStore`]) — zero key, zero DB,
//! full-fidelity dev. Real persistence (Neo4j: native vector index + the entity graph) lives in
//! the control plane behind this same seam (ADR 0026 §2); `neo4j-driver` never enters this crate.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use ailu_rag_pipeline::{cosine_similarity, Embedder, Embedding, MockEmbedder};

/// Attribution threaded through EVERY memory write (ADR 0026 §1). A `false`-ish field stays off
/// the wire (`skip_serializing_if`), so a minimal provenance is cheap.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Who wrote it — an agent id or a human principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// The document this memory was extracted from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_doc_id: Option<String>,
    /// Ed25519 attestation-chain entry id (governed writes; control-plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation_id: Option<String>,
    /// ISO-8601 write time (stamped by the caller — the engine has no clock).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extracted_at: String,
    /// Extractor confidence (0..1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// `"asserted" | "verified" | "rejected"` — the governed review lifecycle (control-plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// A recallable memory item (the M3 agent-memory unit + the vector-recall unit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    pub namespace: String,
    pub key: String,
    /// The recallable content (what gets injected into a seed on recall).
    pub text: String,
    /// Semantic embedding for vector recall. `None` items are never vector-ranked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
    #[serde(default)]
    pub provenance: MemoryProvenance,
}

/// A first-class entity node in the knowledge graph (sub-document: person / project / decision /
/// system / policy / … — an open kebab vocabulary). The graph half of "vector + graph".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntity {
    /// Canonical dedup key, e.g. `"person:jane-doe"`.
    pub id: String,
    pub namespace: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub name: String,
    #[serde(default)]
    pub attributes: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub provenance: MemoryProvenance,
}

/// A typed edge between two entities (or doc↔entity via the `mentions` type).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEdge {
    pub namespace: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub provenance: MemoryProvenance,
}

/// Which recall modalities a run uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallMode {
    Vector,
    Graph,
    #[default]
    Both,
}

/// A pure, deterministic retrieval policy (ADR 0026 §1): tenant scope is enforced at the seam
/// (the namespace is supplied at construction), `top_k` bounds the set, ordering is score-desc
/// with an explicit insertion-order tie-break, and results are deduped by id/key.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalPolicy {
    pub top_k: usize,
    #[serde(default)]
    pub mode: RecallMode,
}

impl Default for RetrievalPolicy {
    fn default() -> Self {
        Self {
            top_k: 5,
            mode: RecallMode::Both,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory store error: {0}")]
    Store(String),
}

/// The DB-free memory seam: vector (semantic) recall + entity-graph recall + provenance-tagged
/// writes. Object-safe (`Arc<dyn MemoryStore>`); the control plane plugs a Neo4j impl behind it.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist (or upsert by `(namespace, key)`) a recallable item.
    async fn put_item(&self, item: MemoryItem) -> Result<(), MemoryError>;

    /// Vector recall: the `top_k` items in `namespace` nearest to `query` by cosine, ranked
    /// score-desc with a stable insertion-order tie-break. Items without an embedding are skipped.
    async fn recall_by_vector(
        &self,
        namespace: &str,
        query: &[f64],
        top_k: usize,
    ) -> Result<Vec<MemoryItem>, MemoryError>;

    /// Persist (or upsert by canonical id) an entity.
    async fn put_entity(&self, entity: MemoryEntity) -> Result<(), MemoryError>;

    /// Persist a typed edge.
    async fn put_edge(&self, edge: MemoryEdge) -> Result<(), MemoryError>;

    /// Graph recall: entities within `depth` hops of `entity_id` (depth-limited BFS over edges),
    /// in deterministic discovery order (excludes the seed).
    async fn neighbors(
        &self,
        namespace: &str,
        entity_id: &str,
        depth: usize,
    ) -> Result<Vec<MemoryEntity>, MemoryError>;
}

/// The in-memory default — cosine vector recall + adjacency-BFS graph recall, `Mutex`-guarded,
/// zero DB. The OSS dev experience is full-fidelity; the control plane swaps in Neo4j.
#[derive(Default)]
pub struct InMemoryMemoryStore {
    // Insertion-ordered so the tie-break is deterministic; keyed maps for upsert.
    items: Mutex<Vec<MemoryItem>>,
    entities: Mutex<Vec<MemoryEntity>>,
    edges: Mutex<Vec<MemoryEdge>>,
}

impl InMemoryMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemoryStore {
    async fn put_item(&self, item: MemoryItem) -> Result<(), MemoryError> {
        let mut items = self.items.lock().expect("memory items mutex");
        if let Some(existing) = items
            .iter_mut()
            .find(|i| i.namespace == item.namespace && i.key == item.key)
        {
            *existing = item;
        } else {
            items.push(item);
        }
        Ok(())
    }

    async fn recall_by_vector(
        &self,
        namespace: &str,
        query: &[f64],
        top_k: usize,
    ) -> Result<Vec<MemoryItem>, MemoryError> {
        let items = self.items.lock().expect("memory items mutex");
        // Score in insertion order; a stable sort keeps that order as the tie-break for equal scores.
        let mut scored: Vec<(f64, &MemoryItem)> = items
            .iter()
            .filter(|i| i.namespace == namespace)
            .filter_map(|i| {
                i.embedding
                    .as_ref()
                    .map(|e| (cosine_similarity(query, e), i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(top_k)
            .map(|(_, i)| i.clone())
            .collect())
    }

    async fn put_entity(&self, entity: MemoryEntity) -> Result<(), MemoryError> {
        let mut entities = self.entities.lock().expect("memory entities mutex");
        if let Some(existing) = entities
            .iter_mut()
            .find(|e| e.namespace == entity.namespace && e.id == entity.id)
        {
            *existing = entity;
        } else {
            entities.push(entity);
        }
        Ok(())
    }

    async fn put_edge(&self, edge: MemoryEdge) -> Result<(), MemoryError> {
        self.edges.lock().expect("memory edges mutex").push(edge);
        Ok(())
    }

    async fn neighbors(
        &self,
        namespace: &str,
        entity_id: &str,
        depth: usize,
    ) -> Result<Vec<MemoryEntity>, MemoryError> {
        let edges = self.edges.lock().expect("memory edges mutex");
        let entities = self.entities.lock().expect("memory entities mutex");
        // Depth-limited BFS over edges (undirected adjacency), deterministic discovery order.
        let mut visited: Vec<String> = vec![entity_id.to_owned()];
        let mut frontier: Vec<String> = vec![entity_id.to_owned()];
        for _ in 0..depth {
            let mut next: Vec<String> = Vec::new();
            for node in &frontier {
                for edge in edges.iter().filter(|e| e.namespace == namespace) {
                    let other = if &edge.from == node {
                        Some(&edge.to)
                    } else if &edge.to == node {
                        Some(&edge.from)
                    } else {
                        None
                    };
                    if let Some(other) = other {
                        if !visited.contains(other) && !next.contains(other) {
                            next.push(other.clone());
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            visited.extend(next.iter().cloned());
            frontier = next;
        }
        // Resolve discovered ids (skip the seed) to entities, preserving discovery order.
        Ok(visited
            .into_iter()
            .skip(1)
            .filter_map(|id| {
                entities
                    .iter()
                    .find(|e| e.namespace == namespace && e.id == id)
                    .cloned()
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ns: &str, key: &str, text: &str, emb: Vec<f64>) -> MemoryItem {
        MemoryItem {
            namespace: ns.to_owned(),
            key: key.to_owned(),
            text: text.to_owned(),
            embedding: Some(emb),
            provenance: MemoryProvenance {
                principal: Some("agent:test".to_owned()),
                extracted_at: "2026-06-24T00:00:00Z".to_owned(),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn vector_recall_ranks_by_cosine_top_k() {
        let store = InMemoryMemoryStore::new();
        store
            .put_item(item("ns", "a", "cats", vec![1.0, 0.0]))
            .await
            .unwrap();
        store
            .put_item(item("ns", "b", "dogs", vec![0.0, 1.0]))
            .await
            .unwrap();
        store
            .put_item(item("ns", "c", "felines", vec![0.9, 0.1]))
            .await
            .unwrap();
        let hits = store.recall_by_vector("ns", &[1.0, 0.0], 2).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].key, "a"); // exact match
        assert_eq!(hits[1].key, "c"); // closest
    }

    #[tokio::test]
    async fn vector_recall_is_namespace_scoped_and_skips_embeddingless() {
        let store = InMemoryMemoryStore::new();
        store
            .put_item(item("ns", "a", "x", vec![1.0, 0.0]))
            .await
            .unwrap();
        store
            .put_item(item("other", "b", "y", vec![1.0, 0.0]))
            .await
            .unwrap();
        let mut no_emb = item("ns", "c", "z", vec![]);
        no_emb.embedding = None;
        store.put_item(no_emb).await.unwrap();
        let hits = store.recall_by_vector("ns", &[1.0, 0.0], 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "a");
    }

    #[tokio::test]
    async fn put_item_upserts_by_namespace_key() {
        let store = InMemoryMemoryStore::new();
        store
            .put_item(item("ns", "a", "old", vec![1.0]))
            .await
            .unwrap();
        store
            .put_item(item("ns", "a", "new", vec![1.0]))
            .await
            .unwrap();
        let hits = store.recall_by_vector("ns", &[1.0], 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "new");
    }

    fn entity(ns: &str, id: &str) -> MemoryEntity {
        MemoryEntity {
            id: id.to_owned(),
            namespace: ns.to_owned(),
            entity_type: "project".to_owned(),
            name: id.to_owned(),
            attributes: serde_json::Map::new(),
            provenance: MemoryProvenance::default(),
        }
    }
    fn edge(ns: &str, from: &str, to: &str) -> MemoryEdge {
        MemoryEdge {
            namespace: ns.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            edge_type: "relates-to".to_owned(),
            provenance: MemoryProvenance::default(),
        }
    }

    #[tokio::test]
    async fn graph_recall_bfs_depth_limited() {
        let store = InMemoryMemoryStore::new();
        for id in ["a", "b", "c", "d"] {
            store.put_entity(entity("ns", id)).await.unwrap();
        }
        store.put_edge(edge("ns", "a", "b")).await.unwrap();
        store.put_edge(edge("ns", "b", "c")).await.unwrap();
        store.put_edge(edge("ns", "c", "d")).await.unwrap();
        // depth 1 from a → just b
        let d1 = store.neighbors("ns", "a", 1).await.unwrap();
        assert_eq!(
            d1.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
        // depth 2 from a → b, c (discovery order)
        let d2 = store.neighbors("ns", "a", 2).await.unwrap();
        assert_eq!(
            d2.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[test]
    fn provenance_skips_empty_fields_on_the_wire() {
        let value = serde_json::to_value(MemoryProvenance::default()).unwrap();
        assert_eq!(value, serde_json::json!({}));
    }
}
