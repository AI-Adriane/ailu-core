//! DSL-level validation of the graph AST — Rust mirror of
//! `packages/graph-ailu/src/validator/{types,validate-graph-ast}.ts`.

use std::collections::{HashMap, HashSet};

use crate::ast::{GraphAst, Loc};
use crate::parser::is_valid_semver;
use serde::{Deserialize, Serialize};

/// Stable machine-readable diagnostic codes — the TS validator's string codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticCode {
    EntryNodeNotFound,
    EdgeNodeNotFound,
    EdgeConditionEmpty,
    SubgraphRefRequired,
    SubgraphRefVersionInvalid,
    /// Kept for parity with the TS diagnostic vocabulary. Unreachable through
    /// the YAML pipeline: the Rust AST types the reducer as an enum and the
    /// parser normalizes unknown reducers to `replace`, exactly like TS.
    ChannelReducerInvalid,
    CycleWithoutRecursionLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding — mirrors the TS `Diagnostic` shape
/// (`{ code, message, loc, severity }`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub loc: Loc,
    pub severity: Severity,
}

fn has_cycle(ast: &GraphAst) -> bool {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &ast.nodes {
        adj.entry(node.id.as_str()).or_default();
    }
    for edge in &ast.edges {
        adj.entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }

    let mut visiting: HashSet<&str> = HashSet::new();
    let mut visited: HashSet<&str> = HashSet::new();

    fn dfs<'a>(
        id: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> bool {
        if visiting.contains(id) {
            return true;
        }
        if visited.contains(id) {
            return false;
        }
        visiting.insert(id);
        if let Some(nexts) = adj.get(id) {
            for next in nexts {
                if dfs(next, adj, visiting, visited) {
                    return true;
                }
            }
        }
        visiting.remove(id);
        visited.insert(id);
        false
    }

    ast.nodes
        .iter()
        .any(|node| dfs(node.id.as_str(), &adj, &mut visiting, &mut visited))
}

/// Validate the AST against the DSL rules. Mirrors the TS `validateGraphAST`:
/// returns every finding (errors and warnings) rather than stopping at the first.
pub fn validate_graph_ast(ast: &GraphAst) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let node_ids: HashSet<&str> = ast.nodes.iter().map(|node| node.id.as_str()).collect();

    if !node_ids.contains(ast.entry_node_id.as_str()) {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::EntryNodeNotFound,
            message: format!("Entry node '{}' does not exist.", ast.entry_node_id),
            loc: ast.loc.clone(),
            severity: Severity::Error,
        });
    }

    for edge in &ast.edges {
        if !node_ids.contains(edge.from.as_str()) || !node_ids.contains(edge.to.as_str()) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::EdgeNodeNotFound,
                message: format!("Edge '{}' references unknown nodes.", edge.id),
                loc: edge.loc.clone(),
                severity: Severity::Error,
            });
        }
        if let Some(condition) = &edge.condition {
            if condition.value.trim().is_empty() {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::EdgeConditionEmpty,
                    message: format!("Edge '{}' has an empty condition.", edge.id),
                    loc: condition.loc.clone(),
                    severity: Severity::Error,
                });
            }
        }
    }

    for node in &ast.nodes {
        if node.node_type == ailu_graph_core::NodeType::Subgraph {
            match &node.subgraph {
                None => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::SubgraphRefRequired,
                    message: format!("Subgraph node '{}' requires a graph reference.", node.id),
                    loc: node.loc.clone(),
                    severity: Severity::Error,
                }),
                Some(subgraph) if !is_valid_semver(&subgraph.version) => {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::SubgraphRefVersionInvalid,
                        message: format!(
                            "Subgraph ref version '{}' is invalid semver.",
                            subgraph.version
                        ),
                        loc: node.loc.clone(),
                        severity: Severity::Error,
                    });
                }
                Some(_) => {}
            }
        }
    }

    // The TS validator also re-checks each channel's reducer at runtime
    // (CHANNEL_REDUCER_INVALID). In Rust the AST types the reducer as
    // `ChannelReducer`, so an invalid reducer is unrepresentable here; the
    // parser already normalizes unknown reducers to `replace` exactly like TS.

    if has_cycle(ast) && ast.recursion_limit.is_none() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::CycleWithoutRecursionLimit,
            message: "Graph contains cycles but recursionLimit is missing.".to_owned(),
            loc: ast.loc.clone(),
            severity: Severity::Warning,
        });
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::build_graph_ast;
    use serde_json::json;

    #[test]
    fn returns_error_with_loc_when_edge_references_missing_node() {
        let ast = build_graph_ast(
            &json!({
                "id": "g",
                "version": "1.0.0",
                "name": "graph",
                "entryNodeId": "n1",
                "channels": {},
                "nodes": [{ "id": "n1", "type": "action", "label": "N1" }],
                "edges": [{ "id": "e1", "from": "n1", "to": "missing", "type": "default" }]
            }),
            "graph.yaml",
        );
        let diagnostics = validate_graph_ast(&ast);
        let missing = diagnostics
            .iter()
            .find(|d| d.code == DiagnosticCode::EdgeNodeNotFound)
            .expect("EDGE_NODE_NOT_FOUND diagnostic");
        assert_eq!(missing.loc.file, "graph.yaml");
        assert_eq!(missing.severity, Severity::Error);
        assert_eq!(missing.message, "Edge 'e1' references unknown nodes.");
    }

    #[test]
    fn warns_on_cycle_without_recursion_limit_and_not_with_one() {
        let cyclic = json!({
            "id": "g",
            "version": "1.0.0",
            "name": "graph",
            "entryNodeId": "a",
            "channels": {},
            "nodes": [
                { "id": "a", "type": "action", "label": "A" },
                { "id": "b", "type": "action", "label": "B" }
            ],
            "edges": [
                { "id": "e1", "from": "a", "to": "b", "type": "default" },
                { "id": "e2", "from": "b", "to": "a", "type": "default" }
            ]
        });
        let ast = build_graph_ast(&cyclic, "graph.yaml");
        let diagnostics = validate_graph_ast(&ast);
        let warning = diagnostics
            .iter()
            .find(|d| d.code == DiagnosticCode::CycleWithoutRecursionLimit)
            .expect("cycle warning");
        assert_eq!(warning.severity, Severity::Warning);

        let mut with_limit = cyclic.clone();
        with_limit["recursionLimit"] = json!(10);
        let ast = build_graph_ast(&with_limit, "graph.yaml");
        assert!(validate_graph_ast(&ast).is_empty());
    }

    #[test]
    fn flags_subgraph_node_without_graph_reference() {
        let ast = build_graph_ast(
            &json!({
                "id": "g",
                "version": "1.0.0",
                "name": "graph",
                "entryNodeId": "s1",
                "channels": {},
                "nodes": [{ "id": "s1", "type": "subgraph", "label": "Sub" }],
                "edges": []
            }),
            "graph.yaml",
        );
        let diagnostics = validate_graph_ast(&ast);
        assert!(diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::SubgraphRefRequired));
    }
}
