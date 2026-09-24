//! Ailu DSL compiler (Rust) — compiles graph YAML into a validated
//! [`ailu_graph_core::GraphDefinition`]. Rust port of `@ailu/graph-ailu`.
//!
//! The pipeline mirrors the TypeScript one stage for stage:
//! `parser` (lenient raw-YAML -> AST) -> `validator` (DSL diagnostics) ->
//! `transformer` (AST -> `GraphDefinition`) -> `compiler` (orchestration plus a
//! final structural [`ailu_graph_core::validate_graph`] gate). The emitted
//! JSON is byte-equivalent to the TS compiler's output for the same YAML.

#![forbid(unsafe_code)]

pub mod ast;
pub mod compiler;
pub mod parser;
pub mod transformer;
pub mod validator;

pub use ast::{ChannelAst, ConditionAst, EdgeAst, GraphAst, Loc, NodeAst, VersionedRef};
pub use compiler::{compile_graph_yaml, DslError};
pub use parser::{build_graph_ast, is_valid_semver, parse_versioned_ref};
pub use transformer::transform_graph;
pub use validator::{validate_graph_ast, Diagnostic, DiagnosticCode, Severity};
