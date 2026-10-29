use std::collections::BTreeMap;

use ailu_graph_core::{GraphDefinition, GraphStatus, RunId};
use ailu_graph_runtime::{
    sync_handler, GraphRuntime, InMemoryConditionRegistry, InMemoryNodeRegistry, NodeOutput,
    NodeRegistry,
};
use ailu_runtime_bridge::catalog_approvals::{
    filing_plan, resume_problems, FilingInput, ResumeCheckInput,
};
use serde_json::{json, Value};

fn graph() -> GraphDefinition {
    serde_json::from_value(json!({
        "id": "g", "name": "g", "version": "1", "channels": {},
        "nodes": [
            { "id": "relay", "type": "action", "label": "relay" },
            { "id": "review", "type": "human-gate", "label": "review" },
            { "id": "publish", "type": "action", "label": "publish" }
        ],
        "edges": [
            { "id": "e1", "from": "relay", "to": "review", "type": "default" },
            { "id": "e2", "from": "review", "to": "publish", "type": "default" }
        ], "entryNodeId": "relay"
    }))
    .unwrap()
}

fn problems(graph: &GraphDefinition, state: &Value, approvals: Value) -> Vec<String> {
    resume_problems(
        &serde_json::from_value::<ResumeCheckInput>(json!({
            "graph": graph, "state": state, "approvals": approvals
        }))
        .unwrap(),
    )
}

#[tokio::test]
async fn injected_ids_cannot_reuse_an_unrelated_approval_to_publish() {
    for node_injection in [false, true] {
        let graph = graph();
        let mut nodes = InMemoryNodeRegistry::new();
        nodes.register(
            "relay".into(),
            sync_handler(move |_| {
                NodeOutput::update(if node_injection {
                    BTreeMap::from([("__approvalIds".to_owned(), json!(["old-approved"]))])
                } else {
                    BTreeMap::new()
                })
            }),
        );
        nodes.register(
            "publish".into(),
            sync_handler(|_| {
                NodeOutput::update(BTreeMap::from([("published".to_owned(), json!(true))]))
            }),
        );
        let runtime = GraphRuntime::new(graph.clone(), nodes, InMemoryConditionRegistry::new());
        let mut state = serde_json::to_value(
            runtime
                .start(
                    RunId::from("victim"),
                    if node_injection {
                        BTreeMap::new()
                    } else {
                        BTreeMap::from([("__approvalIds".to_owned(), json!(["old-approved"]))])
                    },
                )
                .await
                .unwrap(),
        )
        .unwrap();
        let plan = filing_plan(&FilingInput {
            graph: serde_json::to_value(&graph).unwrap(),
            subgraphs: None,
            state: state.clone(),
            previous_state: None,
        });
        assert_eq!(plan.requests.len(), 1, "the current gate must be filed");
        let request = &plan.requests[0];
        state["channels"]["__approvalIds"] = json!(["new-gate"]);
        let pending = json!({ "status": "pending", "runId": request.run_id,
            "nodeId": request.node_id, "requestedBy": request.requested_by,
            "subject": request.subject });
        assert!(!problems(&graph, &state, json!({"new-gate": pending})).is_empty());
        let mut approved = pending;
        approved["status"] = json!("approved");
        approved["resolvedBy"] = json!("alice");
        assert!(problems(&graph, &state, json!({"new-gate": approved})).is_empty());
        // The authorized request still permits the ordinary host resume and side effect.
        let done = runtime.resume(&RunId::from("victim")).await.unwrap();
        assert_eq!(done.status, GraphStatus::Completed);
        assert_eq!(done.channels.get("published"), Some(&json!(true)));
    }
}

#[test]
fn stored_approval_must_match_the_current_wait() {
    let graph = graph();
    let state = json!({ "runId": "victim", "currentNodeId": "review", "status": "suspended",
        "channels": { "__approvalIds": ["old-approved"] } });
    let valid = json!({ "runId": "victim", "nodeId": "review", "requestedBy": "review",
        "status": "approved", "resolvedBy": "alice", "subject": { "description": "gate:review" } });
    assert!(problems(&graph, &state, json!({"old-approved": valid})).is_empty());
    for (field, value) in [
        ("runId", json!("attacker")),
        ("nodeId", json!("other-gate")),
        ("requestedBy", json!("other-agent")),
        ("subject", json!({"description": "gate:other-gate"})),
        ("status", json!("unknown")),
        ("resolvedBy", json!("")),
        ("runId", Value::Null),
        ("nodeId", Value::Null),
        ("requestedBy", Value::Null),
        ("status", Value::Null),
        ("resolvedBy", Value::Null),
    ] {
        let mut record = valid.clone();
        record[field] = value;
        assert!(
            !problems(&graph, &state, json!({"old-approved": record})).is_empty(),
            "{field}"
        );
    }
}

#[test]
fn malformed_or_duplicate_ids_cannot_be_reinterpreted_as_store_keys() {
    let graph = graph();
    for ids in [
        json!(2),
        json!([2]),
        json!([true]),
        json!([{}]),
        json!([""]),
        json!(["a", "a"]),
        Value::Null,
    ] {
        let state = json!({ "runId": "victim", "currentNodeId": "review", "status": "suspended",
            "channels": { "__approvalIds": ids } });
        assert_eq!(
            problems(&graph, &state, json!({})),
            vec!["the run has invalid approval request IDs"]
        );
    }
}

#[test]
fn every_current_request_needs_its_own_decision_including_the_exact_tool_call() {
    let mut graph_json = serde_json::to_value(graph()).unwrap();
    graph_json["nodes"][0]["type"] = json!("agent");
    graph_json["nodes"][0]["metadata"] = json!({ "agent": { "toolNames": ["refund"] } });
    let graph: GraphDefinition = serde_json::from_value(graph_json.clone()).unwrap();
    let mut state = json!({ "runId": "victim", "currentNodeId": "review", "status": "suspended",
        "channels": { "agentResult": { "approvalRequests": [
            { "subject": "tool:refund", "input": { "amount": 4000 } }
        ] } } });
    let plan = filing_plan(&FilingInput {
        graph: graph_json,
        subgraphs: None,
        state: state.clone(),
        previous_state: None,
    });
    assert_eq!(plan.requests.len(), 2);
    let mut records = json!({});
    for (id, request) in ["tool", "gate"].into_iter().zip(plan.requests) {
        records[id] = json!({ "runId": request.run_id, "nodeId": request.node_id,
            "requestedBy": request.requested_by, "subject": request.subject,
            "status": "approved", "resolvedBy": "alice" });
    }
    state["channels"]["__approvalIds"] = json!(["tool", "gate"]);
    assert!(problems(&graph, &state, records.clone()).is_empty());
    for field in ["input", "callInput", "callKey"] {
        let mut wrong_call = records.clone();
        wrong_call["tool"]["subject"][field] = json!("another call");
        assert!(!problems(&graph, &state, wrong_call).is_empty(), "{field}");
    }
    // A valid tool decision cannot stand in for the gate, or vice versa.
    for only in ["tool", "gate"] {
        state["channels"]["__approvalIds"] = json!([only]);
        assert!(problems(&graph, &state, records.clone())
            .iter()
            .any(|p| p.contains("no recorded request")));
    }
}
