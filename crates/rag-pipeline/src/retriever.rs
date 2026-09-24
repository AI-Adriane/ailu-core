//! Retriever — ties an [`Embedder`] and a [`VectorStore`] together.
//!
//! Ports `retriever/retriever.ts`. The TS `Retriever` implements
//! `Runnable<string, RetrievalResult[]>`; this port implements the Rust
//! [`Runnable`] trait from `ailu-runnable`, so it composes with the rest of
//! the runnable machinery (`then`, `batch`, …) exactly like the TS class
//! composes via `pipe` / `batch`.

use std::sync::Arc;

use ailu_runnable::{Runnable, RunnableError};
use async_trait::async_trait;

use crate::embeddings::Embedder;
use crate::types::RetrievalResult;
use crate::vector_store::VectorStore;

/// Default `topK` when none is supplied, matching the TS `topK = 5`.
pub const DEFAULT_TOP_K: usize = 5;

/// Retrieves chunks relevant to a query string.
///
/// Faithful port of the TS `Retriever`: on `invoke`, embed the query, take the
/// first embedding, and `search` the vector store for the top `top_k`. If the
/// embedder returns no vector for the query, the result is empty.
pub struct Retriever {
    vector_store: Arc<dyn VectorStore>,
    embeddings: Arc<dyn Embedder>,
    top_k: usize,
}

impl Retriever {
    /// Create a retriever over a vector store and embedder with an explicit
    /// `top_k`.
    pub fn new(
        vector_store: Arc<dyn VectorStore>,
        embeddings: Arc<dyn Embedder>,
        top_k: usize,
    ) -> Self {
        Self {
            vector_store,
            embeddings,
            top_k,
        }
    }

    /// Create a retriever with the default `top_k` ([`DEFAULT_TOP_K`]), matching
    /// the TS constructor default.
    pub fn with_default_top_k(
        vector_store: Arc<dyn VectorStore>,
        embeddings: Arc<dyn Embedder>,
    ) -> Self {
        Self::new(vector_store, embeddings, DEFAULT_TOP_K)
    }
}

#[async_trait]
impl Runnable<String, Vec<RetrievalResult>> for Retriever {
    async fn invoke(&self, input: String) -> Result<Vec<RetrievalResult>, RunnableError> {
        let embeddings = self
            .embeddings
            .embed(&[input])
            .await
            .map_err(|e| RunnableError::step(e.to_string()))?;
        // `const [embedding] = await ...; if (embedding === undefined) return [];`
        let Some(embedding) = embeddings.into_iter().next() else {
            return Ok(Vec::new());
        };
        self.vector_store
            .search(&embedding, self.top_k)
            .await
            .map_err(|e| RunnableError::step(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::types::{Chunk, Document};
    use crate::vector_store::InMemoryVectorStore;

    fn chunk(id: &str, content: &str, index: usize) -> Chunk {
        Chunk {
            document: Document::new(id, content),
            source_id: "d1".to_string(),
            chunk_index: index,
        }
    }

    #[tokio::test]
    async fn end_to_end_returns_results() {
        let adapter = Arc::new(MockEmbedder::new());
        let store = Arc::new(InMemoryVectorStore::new());

        let mut c1 = chunk("c1", "critical risk", 0);
        let mut c2 = chunk("c2", "general update", 1);
        let vectors = adapter
            .embed(&[c1.content().to_string(), c2.content().to_string()])
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
