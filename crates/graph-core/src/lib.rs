//! Ailu graph-core (Rust).
//!
//! Pure, framework-agnostic graph data model — the Rust port of
//! `@ailu/graph-core`. Zero I/O, no LLM, no framework: just the typed model,
//! its (de)serialization, and graph validation. Everything else builds on this.

#![forbid(unsafe_code)]

pub mod error;
pub mod ids;
pub mod types;
pub mod validator;

pub use error::{ValidationError, ValidationErrorCode};
pub use ids::{EdgeId, GraphId, NodeId, RunId};
pub use types::{
    ChannelDefinition, ChannelReducer, EdgeDefinition, EdgeType, FailureCategory, FanOut,
    GraphDefinition, GraphState, GraphStatus, MapSubgraph, NodeDefinition, NodeType, RetryPolicy,
};
pub use validator::validate_graph;
