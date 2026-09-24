//! Prompt/agent/chain DSL compiler (Rust) — compiles prompt, agent, and chain
//! YAML into their validated definitions. Rust port of `@ailu/lang-ailu`,
//! and the sibling of `ailu-graph-ailu` (which compiles graph YAML).
//!
//! The pipeline mirrors the TypeScript one stage for stage:
//! `parser` (lenient raw-YAML -> AST) -> `validator` (DSL diagnostics) ->
//! `transformer` (AST -> compiled definition, plus the prompt template engine)
//! -> `compiler` (kind dispatch + orchestration). The emitted JSON is
//! byte-equivalent to the TS compiler's output for the same YAML.

#![forbid(unsafe_code)]

pub mod ast;
pub mod compiler;
pub mod parser;
pub mod transformer;
pub mod validator;

pub use ast::{
    AgentAst, AgentKind, ChainAst, ChainKind, ChainStepAst, ChainStepKind, Loc, PromptAst,
    PromptKind,
};
pub use compiler::{
    compile_agent_yaml, compile_chain_yaml, compile_file, compile_prompt_yaml, detect_kind,
    CompileOutput, DslError, DslKind,
};
pub use parser::{build_agent_ast, build_chain_ast, build_prompt_ast, parse_yaml};
pub use transformer::{
    detect_unresolved_template_variables, render_template, transform_agent, transform_chain,
    transform_prompt, AgentConfig, ChainDefinition, ChainStep, PromptTemplate, RenderResult,
};
pub use validator::{
    validate_agent_ast, validate_chain_ast, validate_prompt_ast, Diagnostic, DiagnosticCode,
    Severity,
};
