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

    // Keep each node's next edge on the heap so graph depth cannot exhaust
    // the native stack. Only nodes on the current DFS path are `visiting`.
    let mut stack = Vec::new();
    for node in &ast.nodes {
        let id = node.id.as_str();
        if visited.contains(id) {
            continue;
        }
        visiting.insert(id);
        stack.push((id, 0));
        while let Some((id, next_index)) = stack.last_mut() {
            let next = adj.get(id).and_then(|nexts| nexts.get(*next_index));
            if let Some(&next) = next {
                *next_index += 1;
                if visiting.contains(next) {
                    return true;
                }
                if !visited.contains(next) {
                    visiting.insert(next);
                    stack.push((next, 0));
                }
            } else {
                visiting.remove(id);
                visited.insert(*id);
                stack.pop();
            }
        }
    }
    false
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

    fn chain(node_count: usize) -> GraphAst {
        build_graph_ast(
            &json!({
                "id": "deep",
                "version": "1.0.0",
                "name": "Deep graph",
                "entryNodeId": "0",
                "nodes": (0..node_count)
                    .map(|id| json!({ "id": id.to_string() }))
                    .collect::<Vec<_>>(),
                "edges": (1..node_count)
                    .map(|id| json!({
                        "id": id.to_string(),
                        "from": (id - 1).to_string(),
                        "to": id.to_string()
                    }))
                    .collect::<Vec<_>>()
            }),
            "deep.yaml",
        )
    }

    fn validate_on_small_stack(ast: GraphAst) -> Vec<Diagnostic> {
        // Graph depth must consume heap space, not native call frames.
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || validate_graph_ast(&ast))
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn validates_a_deep_acyclic_chain_on_a_small_stack() {
        let ast = chain(50_000);
        let mut with_limit = ast.clone();
        with_limit.recursion_limit = Some(10);
        assert!(validate_on_small_stack(ast).is_empty());
        assert!(validate_on_small_stack(with_limit).is_empty());
    }

    #[test]
    fn detects_a_deep_cycle_on_a_small_stack_with_and_without_a_limit() {
        let mut ast = chain(50_000);
        let mut back_edge = ast.edges.last().unwrap().clone();
        back_edge.id = "back".to_owned();
        back_edge.from = ast.nodes.last().unwrap().id.clone();
        back_edge.to = "0".to_owned();
        ast.edges.push(back_edge);

        let mut with_limit = ast.clone();
        with_limit.recursion_limit = Some(10);
        let diagnostics = validate_on_small_stack(ast);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            DiagnosticCode::CycleWithoutRecursionLimit
        );
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert_eq!(
            diagnostics[0].message,
            "Graph contains cycles but recursionLimit is missing."
        );
        assert_eq!(diagnostics[0].loc.file, "deep.yaml");
        assert!(validate_on_small_stack(with_limit).is_empty());
    }

    #[test]
    fn accepts_converging_paths_and_duplicate_edges_without_a_cycle() {
        let ast = build_graph_ast(
            &json!({
                "entryNodeId": "a",
                "nodes": [{ "id": "a" }, { "id": "b" }, { "id": "c" }, { "id": "d" }],
                "edges": [
                    { "from": "a", "to": "b" },
                    { "from": "a", "to": "c" },
                    { "from": "b", "to": "c" },
                    { "from": "b", "to": "d" },
                    { "from": "c", "to": "d" },
                    { "from": "c", "to": "d" }
                ]
            }),
            "graph.yaml",
        );
        assert!(validate_graph_ast(&ast).is_empty());
    }

    #[test]
    fn detects_disconnected_cycles_self_loops_and_reachable_unknown_nodes() {
        let mut ast = chain(3);
        let mut back_edge = ast.edges[1].clone();
        back_edge.id = "back".to_owned();
        back_edge.from = "2".to_owned();
        back_edge.to = "1".to_owned();
        ast.edges.push(back_edge);
        // The entry component is acyclic, but every declared node is checked.
        ast.edges.remove(0);
        assert!(has_cycle(&ast));
        ast.edges.last_mut().unwrap().to = "2".to_owned();
        assert!(has_cycle(&ast));

        // A reachable edge endpoint need not be a declared node. It still
        // participates in traversal while preserving missing-node errors.
        ast.nodes.pop();
        let diagnostics = validate_graph_ast(&ast);
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].code, DiagnosticCode::EdgeNodeNotFound);
        assert_eq!(diagnostics[1].code, DiagnosticCode::EdgeNodeNotFound);
        assert_eq!(
            diagnostics[2].code,
            DiagnosticCode::CycleWithoutRecursionLimit
        );

        // Edge-only components unreachable from declared nodes are not visited.
        ast.nodes.pop();
        assert!(!has_cycle(&ast));
    }

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
