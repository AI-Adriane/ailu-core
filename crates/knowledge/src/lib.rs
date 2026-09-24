//! Knowledge base + knowledge graph for Ailu — the engine-owned model, the pure
//! graph operations, and the [`KnowledgeStore`] seam. Rust port of `@ailu/knowledge`.
//!
//! What lives here (engine, OSS): the data shapes ([`KbDocument`], [`KbRelation`],
//! [`KbGraph`]), the pure graph ops ([`build_edges`], [`build_graph`], [`neighbors`],
//! [`resolve_target_id`]), the [`KnowledgeStore`] trait, and an in-memory implementation
//! (cosine search + graph traversal). What stays in the control plane (product): the
//! Postgres-backed store, tenancy/permissions, OAuth connectors, and outbound activation
//! — they implement [`KnowledgeStore`] and reuse these ops.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use ailu_okf::Relation;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A document stored in the knowledge base (its embedding is carried separately).
/// Mirrors the `KbDocumentDto` and the Open Knowledge Format frontmatter fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDocument {
    pub id: String,
    pub namespace: String,
    pub content: String,
    #[serde(rename = "type")]
    pub doc_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    pub created_at: String,
}

/// A typed edge of the knowledge graph: `from --type--> to` within a namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbGraphEdge {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: String,
}

/// A node of the knowledge graph (a document projected for graph views).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbGraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A namespace's knowledge graph: documents as nodes, typed relations as edges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KbGraph {
    pub nodes: Vec<KbGraphNode>,
    pub edges: Vec<KbGraphEdge>,
}

/// A single semantic-search hit: the matched document and its cosine score (1 = identical).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbSearchHit {
    pub id: String,
    pub content: String,
    pub score: f64,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure graph operations
// ---------------------------------------------------------------------------

/// Resolve an OKF link/relation target (bundle-relative `/x.md` or relative) to an entity
/// id within `namespace` (`<namespace>:<path>`). Mirrors the control plane's resolver so
/// edges built here and there agree.
pub fn resolve_target_id(namespace: &str, target: &str) -> String {
    let path = target.strip_prefix('/').unwrap_or(target);
    format!("{namespace}:{path}")
}

/// Build a document's outgoing edges from its OKF `links` (untyped → `"references"`) and
/// typed `relations`, resolving each target to a node id and de-duplicating by
/// `from|type|to` (first-seen order preserved). The pure core of the control plane's
/// `persistRelations`.
pub fn build_edges(
    namespace: &str,
    from_id: &str,
    links: &[String],
    relations: &[Relation],
) -> Vec<KbGraphEdge> {
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    let candidates = links
        .iter()
        .map(|target| ("references".to_owned(), target.as_str()))
        .chain(
            relations
                .iter()
                .map(|relation| (relation.relation_type.clone(), relation.target.as_str())),
        );
    for (edge_type, target) in candidates {
        let to = resolve_target_id(namespace, target);
        let key = format!("{from_id}|{edge_type}|{to}");
        if seen.insert(key) {
            edges.push(KbGraphEdge {
                from: from_id.to_owned(),
                to,
                edge_type,
            });
        }
    }
    edges
}

/// Assemble a [`KbGraph`] from a namespace's documents (nodes) and edges.
pub fn build_graph(docs: &[KbDocument], edges: &[KbGraphEdge]) -> KbGraph {
    KbGraph {
        nodes: docs
            .iter()
            .map(|doc| KbGraphNode {
                id: doc.id.clone(),
                node_type: doc.doc_type.clone(),
                title: doc.title.clone(),
            })
            .collect(),
        edges: edges.to_vec(),
    }
}

/// A depth-limited subgraph reachable from `id` by following outgoing edges (BFS).
/// Mirrors the control plane's `getNeighbors`.
pub fn neighbors(graph: &KbGraph, id: &str, depth: usize) -> KbGraph {
    let mut adjacency: HashMap<&str, Vec<&KbGraphEdge>> = HashMap::new();
    for edge in &graph.edges {
        adjacency.entry(edge.from.as_str()).or_default().push(edge);
    }
    let mut reached: BTreeSet<String> = BTreeSet::new();
    reached.insert(id.to_owned());
    let mut frontier = vec![id.to_owned()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for node_id in &frontier {
            for edge in adjacency.get(node_id.as_str()).into_iter().flatten() {
                if reached.insert(edge.to.clone()) {
                    next.push(edge.to.clone());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    KbGraph {
        nodes: graph
            .nodes
            .iter()
            .filter(|node| reached.contains(&node.id))
            .cloned()
            .collect(),
        edges: graph
            .edges
            .iter()
            .filter(|edge| reached.contains(&edge.from) && reached.contains(&edge.to))
            .cloned()
            .collect(),
    }
}

/// Cosine similarity between two equal-length vectors; 0.0 when either is degenerate or
/// lengths differ. The in-memory store's ranking primitive.
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

// ---------------------------------------------------------------------------
// KnowledgeStore seam
// ---------------------------------------------------------------------------

/// Persistence seam for the knowledge base + graph. The engine ships an in-memory
/// implementation ([`InMemoryKnowledgeStore`], for `ailu dev` / tests / standalone
/// OSS); the control plane implements a Postgres-backed one. The `graph` / `neighbors`
/// default methods are built on the storage primitives + the pure ops, so an
/// implementation only provides storage.
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    /// Insert-or-replace a document and its embedding.
    async fn put_document(&self, document: KbDocument, embedding: Vec<f64>);
    /// Fetch a document by id within a namespace.
    async fn get_document(&self, namespace: &str, id: &str) -> Option<KbDocument>;
    /// All documents in a namespace (insertion order).
    async fn list_documents(&self, namespace: &str) -> Vec<KbDocument>;
    /// Replace a document's outgoing edges in a namespace.
    async fn set_relations(&self, namespace: &str, from_id: &str, edges: Vec<KbGraphEdge>);
    /// All edges in a namespace.
    async fn list_relations(&self, namespace: &str) -> Vec<KbGraphEdge>;
    /// Top-`k` documents in a namespace by cosine similarity to `query_embedding`.
    async fn search(&self, namespace: &str, query_embedding: &[f64], k: usize) -> Vec<KbSearchHit>;

    /// The namespace's knowledge graph (documents as nodes, relations as edges).
    async fn graph(&self, namespace: &str) -> KbGraph {
        let docs = self.list_documents(namespace).await;
        let edges = self.list_relations(namespace).await;
        build_graph(&docs, &edges)
    }

    /// A depth-limited subgraph reachable from `id`.
    async fn neighbors(&self, namespace: &str, id: &str, depth: usize) -> KbGraph {
        let graph = self.graph(namespace).await;
        neighbors(&graph, id, depth)
    }
}

#[derive(Default)]
struct InMemoryState {
    /// `<namespace>` → insertion-ordered `(document, embedding)`.
    docs: Vec<(KbDocument, Vec<f64>)>,
    /// `<namespace>` → all edges.
    edges: Vec<KbGraphEdge>,
}

/// In-memory [`KnowledgeStore`] — cosine search + graph traversal over a `Mutex`-guarded
/// store. The standalone OSS / dev / test backend; not for production persistence.
#[derive(Default)]
pub struct InMemoryKnowledgeStore {
    state: Mutex<InMemoryState>,
}

impl InMemoryKnowledgeStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl KnowledgeStore for InMemoryKnowledgeStore {
    async fn put_document(&self, document: KbDocument, embedding: Vec<f64>) {
        let mut state = self.state.lock().expect("knowledge store poisoned");
        if let Some(slot) = state.docs.iter_mut().find(|(existing, _)| {
            existing.id == document.id && existing.namespace == document.namespace
        }) {
            *slot = (document, embedding);
        } else {
            state.docs.push((document, embedding));
        }
    }

    async fn get_document(&self, namespace: &str, id: &str) -> Option<KbDocument> {
        let state = self.state.lock().expect("knowledge store poisoned");
        state
            .docs
            .iter()
            .find(|(doc, _)| doc.namespace == namespace && doc.id == id)
            .map(|(doc, _)| doc.clone())
    }

    async fn list_documents(&self, namespace: &str) -> Vec<KbDocument> {
        let state = self.state.lock().expect("knowledge store poisoned");
        state
            .docs
            .iter()
            .filter(|(doc, _)| doc.namespace == namespace)
            .map(|(doc, _)| doc.clone())
            .collect()
    }

    async fn set_relations(&self, _namespace: &str, from_id: &str, edges: Vec<KbGraphEdge>) {
        // Edge ids already encode their namespace (`<namespace>:<path>`), so the flat
        // in-memory store keys outgoing edges by `from` alone: drop this document's prior
        // outgoing edges, then append the new set (deduped by `from|type|to`).
        let mut state = self.state.lock().expect("knowledge store poisoned");
        state.edges.retain(|edge| edge.from != from_id);
        let mut seen: BTreeSet<String> = state.edges.iter().map(edge_key).collect();
        for edge in edges {
            if seen.insert(edge_key(&edge)) {
                state.edges.push(edge);
            }
        }
    }

    async fn list_relations(&self, _namespace: &str) -> Vec<KbGraphEdge> {
        let state = self.state.lock().expect("knowledge store poisoned");
        state.edges.clone()
    }

    async fn search(&self, namespace: &str, query_embedding: &[f64], k: usize) -> Vec<KbSearchHit> {
        let state = self.state.lock().expect("knowledge store poisoned");
        let mut hits: Vec<KbSearchHit> = state
            .docs
            .iter()
            .filter(|(doc, _)| doc.namespace == namespace)
            .map(|(doc, embedding)| KbSearchHit {
                id: doc.id.clone(),
                content: doc.content.clone(),
                score: cosine_similarity(query_embedding, embedding),
                doc_type: Some(doc.doc_type.clone()),
                title: doc.title.clone(),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        hits
    }
}

/// Deterministic de-dup key for an edge (`from|type|to`).
fn edge_key(edge: &KbGraphEdge) -> String {
    format!("{}|{}|{}", edge.from, edge.edge_type, edge.to)
}

/// A stored relation (namespaced edge), used by persistence backends that key edges by
/// namespace. The in-memory store keeps edges flat; a Pg backend carries the namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbRelation {
    pub namespace: String,
    pub from_id: String,
    #[serde(rename = "type")]
    pub relation_type: String,
    pub to_id: String,
}

impl KbRelation {
    /// Project to the graph edge shape.
    pub fn to_edge(&self) -> KbGraphEdge {
        KbGraphEdge {
            from: self.from_id.clone(),
            to: self.to_id.clone(),
            edge_type: self.relation_type.clone(),
        }
    }
}

/// Group edges into a `BTreeMap<from, Vec<edge>>` — a small adjacency helper for backends.
pub fn adjacency(edges: &[KbGraphEdge]) -> BTreeMap<String, Vec<KbGraphEdge>> {
    let mut out: BTreeMap<String, Vec<KbGraphEdge>> = BTreeMap::new();
    for edge in edges {
        out.entry(edge.from.clone()).or_default().push(edge.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, ns: &str, doc_type: &str, title: Option<&str>) -> KbDocument {
        KbDocument {
            id: id.to_owned(),
            namespace: ns.to_owned(),
            content: format!("content of {id}"),
            doc_type: doc_type.to_owned(),
            title: title.map(str::to_owned),
            description: None,
            resource: None,
            timestamp: None,
            path: None,
            tags: None,
            links: None,
            created_at: "0".to_owned(),
        }
    }

    #[test]
    fn resolve_target_id_strips_leading_slash() {
        assert_eq!(resolve_target_id("kb", "/a/b.md"), "kb:a/b.md");
        assert_eq!(resolve_target_id("kb", "rel.md"), "kb:rel.md");
    }

    #[test]
    fn build_edges_from_links_and_relations_deduped() {
        let edges = build_edges(
            "kb",
            "kb:src.md",
            &["/a.md".to_owned(), "a.md".to_owned()],
            &[Relation {
                relation_type: "depends-on".to_owned(),
                target: "/b.md".to_owned(),
            }],
        );
        // Both link forms resolve to the SAME id (kb:a.md) → one "references" edge; plus
        // the typed depends-on edge.
        assert_eq!(
            edges,
            vec![
                KbGraphEdge {
                    from: "kb:src.md".to_owned(),
                    to: "kb:a.md".to_owned(),
                    edge_type: "references".to_owned()
                },
                KbGraphEdge {
                    from: "kb:src.md".to_owned(),
                    to: "kb:b.md".to_owned(),
                    edge_type: "depends-on".to_owned()
                },
            ]
        );
    }

    #[test]
    fn build_graph_projects_documents_to_nodes() {
        let graph = build_graph(
            &[
                doc("a", "kb", "note", Some("Alpha")),
                doc("b", "kb", "doc", None),
            ],
            &[KbGraphEdge {
                from: "a".to_owned(),
                to: "b".to_owned(),
                edge_type: "references".to_owned(),
            }],
        );
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].title.as_deref(), Some("Alpha"));
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn neighbors_does_a_depth_limited_bfs() {
        let graph = KbGraph {
            nodes: vec![
                KbGraphNode {
                    id: "a".to_owned(),
                    node_type: "n".to_owned(),
                    title: None,
                },
                KbGraphNode {
                    id: "b".to_owned(),
                    node_type: "n".to_owned(),
                    title: None,
                },
                KbGraphNode {
                    id: "c".to_owned(),
                    node_type: "n".to_owned(),
                    title: None,
                },
                KbGraphNode {
                    id: "d".to_owned(),
                    node_type: "n".to_owned(),
                    title: None,
                },
            ],
            edges: vec![
                KbGraphEdge {
                    from: "a".to_owned(),
                    to: "b".to_owned(),
                    edge_type: "r".to_owned(),
                },
                KbGraphEdge {
                    from: "b".to_owned(),
                    to: "c".to_owned(),
                    edge_type: "r".to_owned(),
                },
                KbGraphEdge {
                    from: "c".to_owned(),
                    to: "d".to_owned(),
                    edge_type: "r".to_owned(),
                },
            ],
        };
        let depth1 = neighbors(&graph, "a", 1);
        let ids: BTreeSet<&str> = depth1.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"].into_iter().collect());

        let depth2 = neighbors(&graph, "a", 2);
        let ids2: BTreeSet<&str> = depth2.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids2, ["a", "b", "c"].into_iter().collect());
    }

    #[tokio::test]
    async fn in_memory_store_search_ranks_by_cosine() {
        let store = InMemoryKnowledgeStore::new();
        store
            .put_document(
                doc("refunds", "kb", "note", Some("Refunds")),
                vec![1.0, 0.0],
            )
            .await;
        store
            .put_document(
                doc("weather", "kb", "note", Some("Weather")),
                vec![0.0, 1.0],
            )
            .await;
        let hits = store.search("kb", &[1.0, 0.1], 2).await;
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "refunds"); // closest to the query vector
        assert!(hits[0].score > hits[1].score);
    }

    #[tokio::test]
    async fn in_memory_store_builds_graph_and_neighbors() {
        let store = InMemoryKnowledgeStore::new();
        store
            .put_document(doc("kb:a.md", "kb", "note", Some("A")), vec![1.0])
            .await;
        store
            .put_document(doc("kb:b.md", "kb", "note", Some("B")), vec![1.0])
            .await;
        let edges = build_edges("kb", "kb:a.md", &["/b.md".to_owned()], &[]);
        store.set_relations("kb", "kb:a.md", edges).await;

        let graph = store.graph("kb").await;
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].to, "kb:b.md");

        let nbrs = store.neighbors("kb", "kb:a.md", 1).await;
        assert_eq!(nbrs.nodes.len(), 2);
    }

    #[tokio::test]
    async fn set_relations_replaces_prior_outgoing_edges() {
        let store = InMemoryKnowledgeStore::new();
        store
            .set_relations(
                "kb",
                "x",
                build_edges("kb", "x", &["/a.md".to_owned()], &[]),
            )
            .await;
        store
            .set_relations(
                "kb",
                "x",
                build_edges("kb", "x", &["/b.md".to_owned()], &[]),
            )
            .await;
        let edges = store.list_relations("kb").await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, "kb:b.md");
    }
}
