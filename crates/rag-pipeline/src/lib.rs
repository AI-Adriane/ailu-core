//! Rust port of `@ailu-ai/rag-pipeline`.
//!
//! A small, deterministic Retrieval-Augmented-Generation pipeline. The public
//! surface mirrors the TypeScript package's module set, with the same flow:
//!
//! ```text
//! load → split → embed → store → retrieve (→ rerank)
//! ```
//!
//! ## Data shapes ([`types`])
//!
//! [`Document`], [`Chunk`], [`Embedding`], [`SplitConfig`], and
//! [`RetrievalResult`] mirror `types.ts` (serde `camelCase`, wire-compatible).
//!
//! ## Seams (traits)
//!
//! Each stage is a trait so a real backend can replace the offline mock without
//! touching the flow:
//!
//! - [`Embedder`] (`embeddings.rs`) — `embed(texts) -> Vec<Embedding>`. The
//!   bundled [`MockEmbedder`] reproduces the TS `MockEmbeddingsAdapter`
//!   *exactly*: a 4-bucket count vector keyed by `charCode % 4`, so the same
//!   text always yields the same vector.
//! - [`Splitter`] (`splitters.rs`) — `split(doc, config) -> Vec<Chunk>`.
//!   [`RecursiveCharacterSplitter`] and [`TokenSplitter`] port the TS splitters,
//!   including the sentence-boundary and blank-line splitting and the chunk-id
//!   scheme `"{docId}:chunk:{index}"`.
//! - [`VectorStore`] (`vector_store.rs`) — `upsert` / `search`. The
//!   [`InMemoryVectorStore`] scores chunks by [`cosine_similarity`] and returns
//!   the top-k, faithful to the TS in-memory store.
//! - [`Retriever`] (`retriever.rs`) — embeds a query and searches; implements
//!   the `ailu-runnable` [`Runnable`](ailu_runnable::Runnable) trait, like
//!   the TS `Retriever implements Runnable`.
//! - [`Reranker`] (`reranker.rs`) — re-scores results. [`LlmReranker`] ports the
//!   TS `LLMReranker`, routing the model call through `ailu-llm-gateway`.
//! - [`DocumentLoader`] (`loaders.rs`) — markdown / html / pdf / csv / json
//!   loaders with the same path-or-inline fallback as the TS loaders.
//!
//! ## Orchestrator
//!
//! [`RagPipeline`] (`pipeline.rs`) wires the seams together: `index` runs
//! load → split → embed → store, and `retrieve` embeds a query and searches.

#![forbid(unsafe_code)]

mod embeddings;
mod error;
mod loaders;
mod pipeline;
mod reranker;
mod retriever;
mod splitters;
mod types;
mod vector_store;

pub use embeddings::{Embedder, MockEmbedder};
pub use error::RagError;
pub use loaders::{CsvLoader, DocumentLoader, HtmlLoader, JsonLoader, MarkdownLoader, PdfLoader};
pub use pipeline::RagPipeline;
pub use reranker::{LlmReranker, Reranker};
pub use retriever::{Retriever, DEFAULT_TOP_K};
pub use splitters::{RecursiveCharacterSplitter, Splitter, TokenSplitter};
pub use types::{Chunk, Document, Embedding, RetrievalResult, SplitConfig};
pub use vector_store::{cosine_similarity, InMemoryVectorStore, VectorStore};

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_runnable::Runnable;
    use std::sync::Arc;

    /// Mirror of the TS `rag-pipeline.test.ts` "runs retriever pipeline
    /// end-to-end" case, using the public surface only.
    #[tokio::test]
    async fn retriever_pipeline_end_to_end() {
        let adapter = Arc::new(MockEmbedder::new());
        let store = Arc::new(InMemoryVectorStore::new());

        let mut c1 = Chunk {
            document: Document::new("c1", "critical risk"),
            source_id: "d1".to_string(),
            chunk_index: 0,
        };
        let mut c2 = Chunk {
            document: Document::new("c2", "general update"),
            source_id: "d1".to_string(),
            chunk_index: 1,
        };
        let vectors = adapter
            .embed(&["critical risk".to_string(), "general update".to_string()])
            .await
            .unwrap();
        c1.document.embedding = Some(vectors[0].clone());
        c2.document.embedding = Some(vectors[1].clone());
        store.upsert(vec![c1, c2]).await.unwrap();

        let retriever = Retriever::new(store, adapter, 2);
        let results = retriever.invoke("critical".to_string()).await.unwrap();
        assert!(!results.is_empty());
    }
}
