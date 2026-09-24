//! DSL AST -> [`ailu_graph_core::GraphDefinition`] — Rust mirror of
//! `packages/graph-ailu/src/transformer/transform-graph.ts`.
//!
//! The transform is a pure, defaults-free remap: every field already carries
//! its parser-applied default, so this stage only reshapes (channels become a
//! map keyed by name, the subgraph ref collapses to its id, the condition
//! collapses to its string). It never generates edge ids or infers the entry
//! node — the TS transformer does neither, so neither do we.

use std::collections::BTreeMap;

use ailu_graph_core::{ChannelDefinition, EdgeDefinition, GraphDefinition, NodeDefinition};

use crate::ast::GraphAst;

/// Transform a validated graph AST into a [`GraphDefinition`]. Mirror of the TS
/// `transformGraph`: id/version/name/entryNodeId pass through verbatim, channels
/// become a name-keyed map, each node keeps only id/type/label/subgraphId, and
/// each edge keeps only id/from/to/type/condition.
pub fn transform_graph(ast: &GraphAst) -> GraphDefinition {
    let channels: BTreeMap<String, ChannelDefinition> = ast
        .channels
        .iter()
        .map(|channel| {
            (
                channel.name.clone(),
                ChannelDefinition {
                    channel_type: channel.channel_type.clone(),
                    reducer: channel.reducer,
                    default: channel.default.clone(),
                    no_log: false,
                },
            )
        })
        .collect();

    let nodes: Vec<NodeDefinition> = ast
        .nodes
        .iter()
        .map(|node| NodeDefinition {
            id: node.id.as_str().into(),
            node_type: node.node_type,
            label: node.label.clone(),
            subgraph_id: node
                .subgraph
                .as_ref()
                .map(|reference| reference.id.as_str().into()),
            input_mapping: None,
            output_mapping: None,
            fan_out: None,
            map_subgraph: None,
            retry_policy: None,
            metadata: None,
        })
        .collect();

    let edges: Vec<EdgeDefinition> = ast
        .edges
        .iter()
        .map(|edge| EdgeDefinition {
            id: edge.id.as_str().into(),
            from: edge.from.as_str().into(),
            to: edge.to.as_str().into(),
            edge_type: edge.edge_type,
            condition: edge
                .condition
                .as_ref()
                .map(|condition| condition.value.clone()),
        })
        .collect();

    GraphDefinition {
        id: ast.id.as_str().into(),
        version: ast.version.clone(),
        name: ast.name.clone(),
        recursion_limit: ast.recursion_limit,
        channels,
        nodes,
        edges,
        entry_node_id: ast.entry_node_id.as_str().into(),
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::build_graph_ast;
    use ailu_graph_core::ChannelReducer;
    use serde_json::json;

    #[test]
    fn maps_channel_reducer_correctly() {
        let ast = build_graph_ast(
            &json!({
                "id": "g",
                "version": "1.0.0",
                "name": "graph",
                "entryNodeId": "n1",
                "channels": { "ctx": { "type": "object", "reducer": "merge", "default": {} } },
                "nodes": [{ "id": "n1", "type": "action", "label": "N1" }],
                "edges": []
            }),
            "graph.yaml",
        );
        let def = transform_graph(&ast);
        assert_eq!(
            def.channels.get("ctx").unwrap().reducer,
            ChannelReducer::Merge
        );
    }

    #[test]
    fn collapses_subgraph_ref_to_its_id() {
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
        let def = transform_graph(&ast);
        assert_eq!(
            def.nodes[0].subgraph_id.as_ref().unwrap().as_str(),
            "risk-agent"
        );
    }
}
