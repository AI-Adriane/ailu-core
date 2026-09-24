//! Compiler entry point — Rust mirror of
//! `packages/graph-ailu/src/compiler/compile-graph-file.ts`.
//!
//! Pipeline: `serde_yaml` parse -> [`build_graph_ast`] -> [`validate_graph_ast`]
//! (DSL diagnostics) -> on no error-severity diagnostic, [`transform_graph`] ->
//! [`validate_graph`] (structural). The TS compiler stops after the DSL stage;
//! we additionally fold the structural validation in as a final gate so the
//! emitted [`GraphDefinition`] is guaranteed sound (it catches duplicate ids and
//! condition-less conditional edges the DSL pass alone does not).

use ailu_graph_core::{validate_graph, GraphDefinition, ValidationError};

use crate::parser::build_graph_ast;
use crate::transformer::transform_graph;
use crate::validator::{validate_graph_ast, Diagnostic, Severity};

/// Failure modes of [`compile_graph_yaml`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DslError {
    /// The input was not well-formed YAML.
    #[error("failed to parse graph YAML: {0}")]
    Parse(String),
    /// The graph AST failed DSL-level validation. Carries every error-severity
    /// diagnostic (warnings are not fatal and are not collected here).
    #[error("graph DSL validation failed: {}", format_diagnostics(.0))]
    DslValidation(Vec<Diagnostic>),
    /// The transformed graph failed structural validation in `graph-core`.
    #[error("graph structural validation failed: {}", format_validation_errors(.0))]
    StructuralValidation(Vec<ValidationError>),
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{} ({})", diagnostic.message, diagnostic.loc))
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_validation_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|error| error.message.clone())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Compile graph YAML into a validated [`GraphDefinition`].
///
/// Mirrors the TS `compileGraphFile` contract (parse -> DSL-validate -> transform)
/// and folds `graph-core`'s structural [`validate_graph`] in as a final gate. The
/// file label `"graph.yaml"` is attached to every diagnostic's location, matching
/// the default the TS callers pass for inline content.
pub fn compile_graph_yaml(yaml: &str) -> Result<GraphDefinition, DslError> {
    let raw: serde_json::Value =
        serde_yaml::from_str(yaml).map_err(|error| DslError::Parse(error.to_string()))?;

    let ast = build_graph_ast(&raw, "graph.yaml");
    let diagnostics = validate_graph_ast(&ast);
    let errors: Vec<Diagnostic> = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        return Err(DslError::DslValidation(errors));
    }

    let definition = transform_graph(&ast);

    let structural = validate_graph(&definition);
    if !structural.is_empty() {
        return Err(DslError::StructuralValidation(structural));
    }

    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::DiagnosticCode;
    use serde_json::json;

    const HAPPY_PATH: &str = r#"
id: graph-1
version: 1.0.0
name: Demo graph
entryNodeId: n1
channels:
  messages:
    type: messages
    reducer: append
nodes:
  - id: n1
    type: action
    label: Start
edges: []
"#;

    #[test]
    fn compiles_a_valid_graph_to_the_expected_definition() {
        let definition = compile_graph_yaml(HAPPY_PATH).expect("graph should compile");
        let actual = serde_json::to_value(&definition).unwrap();
        let expected = json!({
            "id": "graph-1",
            "version": "1.0.0",
            "name": "Demo graph",
            "channels": {
                "messages": { "type": "messages", "reducer": "append" }
            },
            "nodes": [
                { "id": "n1", "type": "action", "label": "Start" }
            ],
            "edges": [],
            "entryNodeId": "n1"
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn compiles_a_conditional_edge_graph_to_the_expected_definition() {
        let yaml = r#"
id: branch
version: 2.1.0
name: Branching graph
recursionLimit: 5
entryNodeId: start
channels:
  ctx:
    type: object
    reducer: merge
    default: {}
nodes:
  - id: start
    type: action
    label: Start
  - id: approve
    type: human-gate
    label: Approve
  - id: done
    type: action
    label: Done
edges:
  - id: e1
    from: start
    to: approve
    type: default
  - id: e2
    from: approve
    to: done
    type: conditional
    condition: state.approved === true
"#;
        let definition = compile_graph_yaml(yaml).expect("conditional graph should compile");
        let actual = serde_json::to_value(&definition).unwrap();
        let expected = json!({
            "id": "branch",
            "version": "2.1.0",
            "name": "Branching graph",
            "recursionLimit": 5,
            "channels": {
                "ctx": { "type": "object", "reducer": "merge", "default": {} }
            },
            "nodes": [
                { "id": "start", "type": "action", "label": "Start" },
                { "id": "approve", "type": "human-gate", "label": "Approve" },
                { "id": "done", "type": "action", "label": "Done" }
            ],
            "edges": [
                { "id": "e1", "from": "start", "to": "approve", "type": "default" },
                {
                    "id": "e2",
                    "from": "approve",
                    "to": "done",
                    "type": "conditional",
                    "condition": "state.approved === true"
                }
            ],
            "entryNodeId": "start"
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_malformed_yaml() {
        // Unterminated flow mapping — js-yaml/serde_yaml both reject this.
        let result = compile_graph_yaml("id: graph-1\nnodes: [ {id: n1");
        assert!(matches!(result, Err(DslError::Parse(_))));
    }

    #[test]
    fn rejects_a_dsl_validation_failure_for_a_missing_entry_node() {
        let yaml = r#"
id: graph-1
version: 1.0.0
name: Demo
entryNodeId: ghost
channels: {}
nodes:
  - id: n1
    type: action
    label: Start
edges: []
"#;
        let result = compile_graph_yaml(yaml);
        let Err(DslError::DslValidation(diagnostics)) = result else {
            panic!("expected a DSL validation error, got {result:?}");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::EntryNodeNotFound));
    }

    #[test]
    fn rejects_a_dsl_validation_failure_for_an_edge_to_an_unknown_node() {
        let yaml = r#"
id: graph-1
version: 1.0.0
name: Demo
entryNodeId: n1
channels: {}
nodes:
  - id: n1
    type: action
    label: Start
edges:
  - id: e1
    from: n1
    to: missing
    type: default
"#;
        let result = compile_graph_yaml(yaml);
        let Err(DslError::DslValidation(diagnostics)) = result else {
            panic!("expected a DSL validation error, got {result:?}");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::EdgeNodeNotFound));
    }

    #[test]
    fn folds_structural_validation_for_duplicate_node_ids() {
        // The DSL validator does not check for duplicate ids; the structural
        // gate does. This proves the final `validate_graph` fold is wired.
        let yaml = r#"
id: graph-1
version: 1.0.0
name: Demo
entryNodeId: n1
channels: {}
nodes:
  - id: n1
    type: action
    label: Start
  - id: n1
    type: agent
    label: Dup
edges: []
"#;
        let result = compile_graph_yaml(yaml);
        assert!(matches!(result, Err(DslError::StructuralValidation(_))));
    }
}
