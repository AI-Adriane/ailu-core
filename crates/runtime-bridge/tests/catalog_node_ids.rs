use ailu_runtime_bridge::catalog::{
    catalog_spec, catalog_spec_json, spec_from_catalog, CatalogError, CatalogInput,
};
use serde_json::{json, Value};

fn graph(id: &str, nodes: Vec<Value>) -> Value {
    json!({
        "id": id, "version": "1", "name": id, "channels": {},
        "entryNodeId": nodes.first().map(|node| &node["id"]),
        "nodes": nodes, "edges": []
    })
}

fn node_variants(id: &str) -> Vec<Value> {
    vec![
        json!({ "id": id, "type": "action", "label": id }),
        json!({ "id": id, "type": "agent", "label": id,
            "metadata": { "agent": { "model": "selected-model" } } }),
        json!({ "id": id, "type": "action", "label": id,
            "metadata": { "component": { "kind": "textCleaner" } } }),
        json!({ "id": id, "type": "action", "label": id,
            "metadata": { "mapAgents": {
                "overChannel": "items", "joinAt": "out", "subAgent": {}
            } } }),
        json!({ "id": id, "type": "human-gate", "label": id }),
        json!({ "id": id, "type": "subgraph", "label": id, "subgraphId": "child" }),
        json!({ "id": id, "type": "action", "label": id,
            "metadata": { "mapAgents": { "overChannel": "" } } }),
    ]
}

#[test]
fn duplicate_node_ids_are_refused_before_carrier_or_host_binding_resolution() {
    for first in node_variants("shared") {
        for second in node_variants("shared") {
            let layouts = [
                (graph("root", vec![first.clone(), second.clone()]), vec![]),
                (
                    graph("root", vec![first.clone()]),
                    vec![graph("child", vec![second.clone()])],
                ),
                (
                    graph("root", vec![]),
                    vec![graph("child", vec![first.clone(), second.clone()])],
                ),
                (
                    graph("root", vec![]),
                    vec![
                        graph("child", vec![first.clone()]),
                        graph("sibling", vec![second.clone()]),
                    ],
                ),
            ];
            for (graph, subgraphs) in layouts {
                for host_nodes in [None, Some(vec!["shared".to_owned()])] {
                    let input = CatalogInput {
                        graph: graph.clone(),
                        subgraphs: Some(subgraphs.clone()),
                        host_nodes,
                        ..CatalogInput::default()
                    };
                    assert_eq!(
                        catalog_spec(&input),
                        Err(CatalogError::InvalidDefinition(
                            "duplicate node id 'shared'".to_owned()
                        )),
                        "input: {input:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn duplicate_node_ids_are_refused_through_typed_and_json_entry_points() {
    // JSON escapes must be compared after decoding, just as registry keys are.
    let raw = r#"{"graph":{"id":"root","version":"1","name":"root","channels":{},
        "nodes":[{"id":"shared","type":"agent","label":"shared","metadata":{"agent":{}}}],
        "edges":[],"entryNodeId":"shared"},
        "subgraphs":[{"id":"child","version":"1","name":"child","channels":{},
        "nodes":[{"id":"\u0073hared","type":"agent","label":"shared","metadata":{"agent":{"model":"replacement"}}}],
        "edges":[],"entryNodeId":"shared"}]}"#;
    let input: CatalogInput = serde_json::from_str(raw).unwrap();
    assert_eq!(
        spec_from_catalog(&input).unwrap_err(),
        "invalid catalog graph: duplicate node id 'shared'"
    );
    let wire: Value = serde_json::from_str(&catalog_spec_json(raw).unwrap()).unwrap();
    assert_eq!(
        wire,
        json!({ "error": {
        "kind": "invalidDefinition",
        "message": "invalid catalog graph: duplicate node id 'shared'"
    } })
    );
}

#[test]
fn unique_ids_preserve_root_child_and_grandchild_bindings() {
    let input = CatalogInput {
        graph: graph(
            "root",
            vec![
                json!({ "id": "host", "type": "action", "label": "host" }),
                json!({ "id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child" }),
            ],
        ),
        subgraphs: Some(vec![
            graph(
                "child",
                vec![
                    json!({ "id": "agent", "type": "agent", "label": "agent",
                    "metadata": { "agent": { "model": "original" } } }),
                    json!({ "id": "nested", "type": "subgraph", "label": "nested", "subgraphId": "grandchild" }),
                ],
            ),
            graph(
                "grandchild",
                vec![json!({ "id": "child-host", "type": "action", "label": "child-host" })],
            ),
        ]),
        host_nodes: Some(vec!["host".to_owned(), "child-host".to_owned()]),
        ..CatalogInput::default()
    };
    let (spec, warnings) = spec_from_catalog(&input).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(spec.host_node_ids, vec!["host", "child-host"]);
    assert_eq!(spec.agents["agent"].model.as_deref(), Some("original"));
    assert_eq!(spec.subgraphs.len(), 2);
}
