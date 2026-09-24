//! Agent-facing memory tools (ADR 0045 Stage 1b) — make durable memory ACTIONABLE inside the agent
//! loop. `recallMemory` (read) + `rememberMemory` (write) are **built-in, governed** tools: the bridge
//! builds them with the agent's `store` / `embedder` / `namespace` / `principal` **sealed** (never
//! user data), exactly like [`crate::MemoryMiddleware`]. Recall is read-only; remember persists with a
//! `sha256(text)` key, so a write is **idempotent** → re-execution on replay (tools are re-run, only
//! LLM I/O is journaled — ADR 0038) does not diverge. Fail-open: a store/embed error becomes a tool
//! observation, never sinks the run.
//!
//! Durability: the remember handler writes to the engine [`MemoryStore`] seam (in-process by default);
//! the control plane also drains the agent's writes from the reserved `__memoryWrites` channel
//! (emitted by the node handler from [`crate::react::AgentResult::memory_writes`]) into its durable
//! store (Neo4j supersede/forget — ADR 0045 control-plane half).

use std::sync::Arc;

use ailu_memory::{Embedder, MemoryItem, MemoryProvenance, MemoryStore};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::tools::{ToolDefinition, ToolHandler};

/// Reserved output channel: the agent's durable memory write intents this run (a JSON array of
/// `{op, key, text}`), patched by the node handler so the control plane can persist them durably.
pub const MEMORY_WRITES_CHANNEL: &str = "__memoryWrites";

/// The read tool — recall relevant facts from the agent's durable memory.
pub const RECALL_MEMORY_TOOL: &str = "recallMemory";
/// The write tool — persist a durable fact for future runs.
pub const REMEMBER_MEMORY_TOOL: &str = "rememberMemory";

/// One durable memory write intent captured during a run (surfaced via [`MEMORY_WRITES_CHANNEL`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWrite {
    /// `"remember"` (v1). `update`/`forget` are control-plane operations (ADR 0045 Stage 1a/2).
    pub op: String,
    /// Deterministic key (`sha256(text)`) — the control plane upserts/supersedes by it.
    pub key: String,
    pub text: String,
}

async fn embed_one(embedder: &Arc<dyn Embedder>, text: &str) -> Option<Vec<f64>> {
    embedder
        .embed(&[text.to_owned()])
        .await
        .ok()
        .and_then(|vectors| vectors.into_iter().next())
}

/// Build the governed memory tools for an agent, sealed with its store/embedder/namespace/principal
/// (the bridge supplies them — never user data). Returns `(definition, handler)` pairs to register in
/// the agent's tool registry. The JSON schema makes them native tool-calls the LLM can emit.
pub fn build_memory_tools(
    store: Arc<dyn MemoryStore>,
    embedder: Arc<dyn Embedder>,
    namespace: String,
    principal: Option<String>,
) -> Vec<(ToolDefinition, ToolHandler)> {
    let recall_def = ToolDefinition {
        name: RECALL_MEMORY_TOOL.to_owned(),
        description:
            "Recall relevant facts you have remembered from earlier (your durable memory)."
                .to_owned(),
        requires_approval: false,
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to look up in your memory." },
                "topK": { "type": "integer", "description": "Max results (default 5)." }
            },
            "required": ["query"]
        })),
        content_scoped: false,
    };
    let recall_store = store.clone();
    let recall_embedder = embedder.clone();
    let recall_ns = namespace.clone();
    let recall_handler: ToolHandler = Box::new(move |input: Value| {
        let store = recall_store.clone();
        let embedder = recall_embedder.clone();
        let namespace = recall_ns.clone();
        Box::pin(async move {
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return Ok(json!({ "items": [] }));
            }
            let top_k = input
                .get("topK")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .max(1) as usize;
            // Fail-open: a missing embedder/store yields an empty recall, never an error that the
            // agent must handle.
            let Some(q) = embed_one(&embedder, query).await else {
                return Ok(json!({ "items": [] }));
            };
            let items = store
                .recall_by_vector(&namespace, &q, top_k)
                .await
                .map(|hits| {
                    hits.into_iter()
                        .map(|h| json!({ "key": h.key, "text": h.text }))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(json!({ "items": items }))
        })
    });

    let remember_def = ToolDefinition {
        name: REMEMBER_MEMORY_TOOL.to_owned(),
        description:
            "Persist a durable fact to your memory so future runs can recall it. Use sparingly — for \
             stable, reusable facts, not transient chatter."
                .to_owned(),
        requires_approval: false,
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The fact to remember (a concise statement)." }
            },
            "required": ["text"]
        })),
        content_scoped: false,
    };
    let remember_store = store;
    let remember_embedder = embedder;
    let remember_ns = namespace;
    let remember_principal = principal;
    let remember_handler: ToolHandler = Box::new(move |input: Value| {
        let store = remember_store.clone();
        let embedder = remember_embedder.clone();
        let namespace = remember_ns.clone();
        let principal = remember_principal.clone();
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            if text.is_empty() {
                return Err("rememberMemory: `text` is required".to_owned());
            }
            // Deterministic key → idempotent on replay (the control plane upserts/supersedes by it).
            let key = format!("{:x}", Sha256::digest(text.as_bytes()));
            let embedding = embed_one(&embedder, &text).await;
            let item = MemoryItem {
                namespace,
                key: key.clone(),
                text,
                embedding,
                provenance: MemoryProvenance {
                    principal,
                    status: Some("asserted".to_owned()),
                    ..Default::default()
                },
            };
            // Fail-open: the in-process write is best-effort; the durable write also goes through the
            // control-plane drain of the __memoryWrites channel.
            let _ = store.put_item(item).await;
            Ok(json!({ "remembered": true, "key": key }))
        })
    });

    vec![
        (recall_def, recall_handler),
        (remember_def, remember_handler),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailu_memory::{InMemoryMemoryStore, MockEmbedder};

    fn tools() -> Vec<(ToolDefinition, ToolHandler)> {
        build_memory_tools(
            Arc::new(InMemoryMemoryStore::new()),
            Arc::new(MockEmbedder),
            "tenant:t1:agent:a".to_owned(),
            Some("agent:a".to_owned()),
        )
    }

    #[tokio::test]
    async fn remember_persists_then_recall_finds_it() {
        let store: Arc<dyn MemoryStore> = Arc::new(InMemoryMemoryStore::new());
        let built = build_memory_tools(
            store.clone(),
            Arc::new(MockEmbedder),
            "ns".to_owned(),
            Some("agent:a".to_owned()),
        );
        let remember = &built[1].1;
        let recall = &built[0].1;

        let out = remember(json!({ "text": "the capital of France is Paris" }))
            .await
            .unwrap();
        assert_eq!(out.get("remembered").and_then(Value::as_bool), Some(true));
        let key = out.get("key").and_then(Value::as_str).unwrap().to_owned();
        assert_eq!(key.len(), 64); // sha256 hex

        // Idempotent: remembering the same text yields the same key (replay-safe).
        let out2 = remember(json!({ "text": "the capital of France is Paris" }))
            .await
            .unwrap();
        assert_eq!(out2.get("key").and_then(Value::as_str), Some(key.as_str()));

        let recalled = recall(json!({ "query": "what is the capital of France?" }))
            .await
            .unwrap();
        let items = recalled.get("items").and_then(Value::as_array).unwrap();
        assert!(items.iter().any(|i| i
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains("Paris"))));
    }

    #[tokio::test]
    async fn remember_requires_text_and_recall_is_tolerant() {
        let built = tools();
        assert!(built[1].1(json!({})).await.is_err()); // remember without text → error
                                                       // recall with an empty/missing query → empty, never an error.
        let empty = built[0].1(json!({ "query": "" })).await.unwrap();
        assert_eq!(
            empty.get("items").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn definitions_are_native_callable_and_ungated() {
        let built = tools();
        for (def, _) in &built {
            assert!(def.input_schema.is_some()); // advertised to the LLM
            assert!(!def.requires_approval); // v1: memory tools are not gated
        }
    }
}
