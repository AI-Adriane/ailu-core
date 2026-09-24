//! Lenient raw-YAML → AST normalization — Rust mirror of
//! `packages/graph-ailu/src/parser/{ref,build-graph-ast}.ts`.
//!
//! The parser never fails: like the TS `buildGraphAST`, every missing or
//! ill-typed field falls back to the same default (`""` for strings, `action`
//! node type, `default` edge type, `replace` reducer, `"unknown"` channel
//! type), and the validator reports what is actually wrong.

use ailu_graph_core::{ChannelReducer, EdgeType, NodeType};
use serde_json::Value;

use crate::ast::{ChannelAst, ConditionAst, EdgeAst, GraphAst, Loc, NodeAst, VersionedRef};

/// Parse a `<id>@<major.minor.patch>` reference. Mirror of the TS
/// `parseVersionedRef` (`/^([^@]+)@(\d+\.\d+\.\d+)$/` over the trimmed value).
pub fn parse_versioned_ref(value: &str) -> Option<VersionedRef> {
    let trimmed = value.trim();
    let (id, version) = trimmed.split_once('@')?;
    if id.is_empty() || !is_valid_semver(version) {
        return None;
    }
    Some(VersionedRef {
        id: id.to_owned(),
        version: version.to_owned(),
    })
}

/// `/^\d+\.\d+\.\d+$/` — three dot-separated runs of ASCII digits.
pub fn is_valid_semver(version: &str) -> bool {
    let mut parts = version.split('.');
    let is_digits = |part: Option<&str>| {
        part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    is_digits(parts.next())
        && is_digits(parts.next())
        && is_digits(parts.next())
        && parts.next().is_none()
}

fn as_string_or_empty(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}

fn normalize_reducer(value: Option<&Value>) -> ChannelReducer {
    match value.and_then(Value::as_str) {
        Some("append") => ChannelReducer::Append,
        Some("merge") => ChannelReducer::Merge,
        _ => ChannelReducer::Replace,
    }
}

fn normalize_node_type(value: Option<&Value>) -> NodeType {
    match value.and_then(Value::as_str) {
        Some("agent") => NodeType::Agent,
        Some("tool") => NodeType::Tool,
        Some("human-gate") => NodeType::HumanGate,
        Some("subgraph") => NodeType::Subgraph,
        _ => NodeType::Action,
    }
}

fn normalize_edge_type(value: Option<&Value>) -> EdgeType {
    match value.and_then(Value::as_str) {
        Some("conditional") => EdgeType::Conditional,
        _ => EdgeType::Default,
    }
}

/// Build the graph AST from raw parsed YAML. Mirror of the TS `buildGraphAST`.
pub fn build_graph_ast(raw: &Value, file: &str) -> GraphAst {
    let loc = || Loc::start_of(file);
    let input = raw.as_object();
    let get = |key: &str| input.and_then(|map| map.get(key));

    let empty_vec: Vec<Value> = Vec::new();
    let nodes_raw = get("nodes").and_then(Value::as_array).unwrap_or(&empty_vec);
    let edges_raw = get("edges").and_then(Value::as_array).unwrap_or(&empty_vec);

    let channels = get("channels")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, definition)| {
                    let def = definition.as_object();
                    let field = |key: &str| def.and_then(|map| map.get(key));
                    ChannelAst {
                        name: name.clone(),
                        channel_type: field("type")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned(),
                        reducer: normalize_reducer(field("reducer")),
                        default: field("default").cloned(),
                        loc: loc(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let nodes = nodes_raw
        .iter()
        .map(|node_raw| {
            let node = node_raw.as_object();
            let field = |key: &str| node.and_then(|map| map.get(key));
            let subgraph = field("graph")
                .and_then(Value::as_str)
                .and_then(parse_versioned_ref);
            NodeAst {
                id: as_string_or_empty(field("id")),
                node_type: normalize_node_type(field("type")),
                label: as_string_or_empty(field("label")),
                subgraph,
                loc: loc(),
            }
        })
        .collect();

    let edges = edges_raw
        .iter()
        .map(|edge_raw| {
            let edge = edge_raw.as_object();
            let field = |key: &str| edge.and_then(|map| map.get(key));
            let condition = field("condition")
                .and_then(Value::as_str)
                .map(|value| ConditionAst {
                    value: value.to_owned(),
                    loc: loc(),
                });
            EdgeAst {
                id: as_string_or_empty(field("id")),
                from: as_string_or_empty(field("from")),
                to: as_string_or_empty(field("to")),
                edge_type: normalize_edge_type(field("type")),
                condition,
                loc: loc(),
            }
        })
        .collect();

    GraphAst {
        id: as_string_or_empty(get("id")),
        version: as_string_or_empty(get("version")),
        name: as_string_or_empty(get("name")),
        recursion_limit: get("recursionLimit")
            .and_then(Value::as_u64)
            .and_then(|limit| u32::try_from(limit).ok()),
        entry_node_id: as_string_or_empty(get("entryNodeId")),
        channels,
        nodes,
        edges,
        loc: loc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_subgraph_versioned_reference() {
        let ast = build_graph_ast(
            &json!({
                "id": "g",
                "version": "1.0.0",
                "name": "x",
                "entryNodeId": "s1",
                "channels": {},
                "nodes": [{ "id": "s1", "type": "subgraph", "label": "sub", "graph": "risk-agent@1.0.0" }],
                "edges": []
            }),
            "graph.yaml",
        );
        assert_eq!(
            ast.nodes[0].subgraph,
            Some(VersionedRef {
                id: "risk-agent".to_owned(),
                version: "1.0.0".to_owned()
            })
        );
    }

    #[test]
    fn applies_the_ts_parser_defaults() {
        let ast = build_graph_ast(&json!({ "nodes": [{ "id": "only" }] }), "min.yaml");
        assert_eq!(ast.id, "");
        assert_eq!(ast.version, "");
        assert_eq!(ast.name, "");
        assert_eq!(ast.entry_node_id, "");
        assert_eq!(ast.recursion_limit, None);
        assert_eq!(ast.nodes[0].node_type, NodeType::Action);
        assert_eq!(ast.nodes[0].label, "");
        assert!(ast.channels.is_empty());
        assert!(ast.edges.is_empty());
    }

    #[test]
    fn rejects_malformed_versioned_refs() {
        assert_eq!(parse_versioned_ref("risk-agent"), None);
        assert_eq!(parse_versioned_ref("@1.0.0"), None);
        assert_eq!(parse_versioned_ref("risk-agent@1.0"), None);
        assert_eq!(parse_versioned_ref("risk-agent@1.0.0.0"), None);
        assert_eq!(parse_versioned_ref("a@b@1.0.0"), None);
        assert!(parse_versioned_ref("  risk-agent@1.0.0  ").is_some());
    }
}
