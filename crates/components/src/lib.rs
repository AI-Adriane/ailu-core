#![forbid(unsafe_code)]

//! Reusable graph components and prebuilt micro-task agents for the Ailu engine.
//!
//! This crate is the Rust core of Ailu's "component library" (the Haystack-style
//! building blocks) and its prebuilt micro-agents. It is shared by every language SDK
//! (TS via napi, Python via pyo3) so a component authored once runs everywhere.
//!
//! Two public surfaces:
//!
//! - **Components** ([`ComponentRegistry`], [`ComponentError`]) — pure (no-LLM)
//!   compute building blocks addressable by a string `kind` + a `serde_json`
//!   `params` object. A graph node declaring `{ component: { kind, params } }` is
//!   compiled into a runtime `NodeHandler` by
//!   [`ComponentRegistry::build_handler`]. Ships `promptBuilder`, `jsonValidator`,
//!   `outputParser`, `router`, `retriever`, `reranker`, `textCleaner`,
//!   `documentSplitter`, `htmlToText`, `csvParser`, `documentJoiner`,
//!   `deduplicator`, `truncator`, `regexExtractor`, `answerBuilder`,
//!   `fieldMapper`, `bm25Retriever`, `keywordRetriever`,
//!   `sentenceWindowSplitter`, `languageDetector`, `metadataFilter`,
//!   `listJoiner`, `mergeRanker`, `evaluator`, `chatMessageBuilder`,
//!   `conditionalRouter`, and `documentWriter`.
//! - **Prebuilt agents** ([`PrebuiltAgent`], [`prebuilt`], [`list_prebuilt`]) —
//!   tier-tagged micro-agent *definitions* (system prompt, [`ModelTier`], tool
//!   names, approval gate, output channel). The concrete model is resolved later
//!   by the `ModelPolicy` in `ailu-llm-gateway`.

mod components;
mod error;
mod prebuilt;

pub use components::ComponentRegistry;
pub use error::ComponentError;
pub use prebuilt::{list_prebuilt, prebuilt, PrebuiltAgent};

// Re-export `ModelTier` so consumers can name a prebuilt agent's tier without
// also depending on `ailu-llm-gateway` directly.
pub use ailu_llm_gateway::ModelTier;
