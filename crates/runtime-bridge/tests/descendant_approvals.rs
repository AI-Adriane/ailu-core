//! Governed resumes must decide every descendant gate before any host action runs.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use ailu_runtime_bridge::{
    catalog_approvals::{filing_plan, resume_problems, FilingInput, ResumeCheckInput},
    run, BridgeResult, Entry, HostCallbacks,
};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Default)]
struct Host(AtomicUsize);

#[async_trait]
impl HostCallbacks for Host {
    async fn on_node(&self, _: Value) -> BridgeResult<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("{}".to_owned())
    }
    fn on_condition(&self, _: Value) -> BridgeResult<bool> {
        Ok(false)
    }
    fn on_event(&self, _: String) {}
}

fn graph(id: &str, nodes: Value, edges: Value, entry: &str) -> Value {
    json!({ "id": id, "version": "1", "name": id, "channels": {},
        "nodes": nodes, "edges": edges, "entryNodeId": entry })
}

fn fixture(mapped: bool, nested: bool) -> Value {
    let leaf = graph(
        "leaf",
        json!([
            {"id":"review", "type":"human-gate", "label":"review"},
            {"id":"publish", "type":"action", "label":"publish"}
        ]),
        json!([{"id":"e", "from":"review", "to":"publish", "type":"default"}]),
        "review",
    );
    let middle = graph(
        "middle",
        json!([
            {"id":"inner", "type":"subgraph", "label":"inner", "subgraphId":"leaf"}
        ]),
        json!([]),
        "inner",
    );
    let mut node = json!({"id":"sub", "type":"subgraph", "label":"sub",
        "subgraphId": if nested {"middle"} else {"leaf"}});
    if mapped {
        node["mapSubgraph"] = json!({"overChannel":"items", "joinAt":"out"});
    }
    let mut parent = graph("parent", json!([node]), json!([]), "sub");
    parent["channels"] = json!({"items":{"type":"json", "reducer":"replace"},
        "out":{"type":"json", "reducer":"replace"}});
    json!({"graph":parent, "subgraphs":[middle, leaf], "runId":"root",
        "initialData":{"items":[{}, {}]}, "hostNodeIds":["publish"]})
}

fn plan(spec: &Value, state: &Value) -> ailu_runtime_bridge::catalog_approvals::FilingPlan {
    filing_plan(
        &serde_json::from_value::<FilingInput>(json!({
            "graph":spec["graph"], "subgraphs":spec["subgraphs"], "state":state
        }))
        .unwrap(),
    )
}

fn problems(spec: &Value, state: &Value, approvals: &Value) -> Vec<String> {
    resume_problems(
        &serde_json::from_value::<ResumeCheckInput>(json!({
            "graph":spec["graph"], "subgraphs":spec["subgraphs"], "state":state,
            "approvals":approvals
        }))
        .unwrap(),
    )
}

async fn assert_governed_resume(mapped: bool, nested: bool, root_status: &str) {
    let mut spec = fixture(mapped, nested);
    let host = Arc::new(Host::default());
    let outcome: Value = serde_json::from_str(
        &run(spec.to_string(), host.clone(), Entry::Start)
            .await
            .unwrap(),
    )
    .unwrap();
    let mut state = outcome["state"].clone();
    assert_eq!(state["status"], "suspended");
    state["status"] = json!(root_status);
    assert_eq!(host.0.load(Ordering::SeqCst), 0);
    assert!(
        !problems(&spec, &state, &json!({})).is_empty(),
        "an unfiled descendant gate passed"
    );
    let filing = plan(&spec, &state);
    assert_eq!(filing.refusal, None);
    assert_eq!(filing.requests.len(), if mapped { 2 } else { 1 });
    let mut approvals = json!({});
    let mut ids = Vec::new();
    for (index, request) in filing.requests.iter().enumerate() {
        let child = if mapped {
            format!("root:sub:{index}")
        } else {
            "root:sub".to_owned()
        };
        let child = if nested {
            format!("{child}:inner")
        } else {
            child
        };
        assert_eq!(request.run_id, child);
        assert_eq!(request.node_id, format!("{child}:review"));
        assert_eq!(request.subject.description, format!("gate:{child}:review"));
        let id = format!("id-{index}");
        approvals[&id] = json!({"status":"pending", "runId":request.run_id,
                "nodeId":request.node_id, "requestedBy":request.requested_by,
                "subject":request.subject});
        ids.push(id);
    }
    state["channels"]["__approvalIds"] = json!(ids);
    assert!(!problems(&spec, &state, &approvals).is_empty());
    for id in &ids {
        approvals[id]["status"] = json!("approved");
        approvals[id]["resolvedBy"] = json!("alice");
    }
    assert!(problems(&spec, &state, &approvals).is_empty());
    let first = &ids[0];
    for status in ["pending", "rejected", "unknown"] {
        let mut bad = approvals.clone();
        bad[first]["status"] = json!(status);
        assert!(!problems(&spec, &state, &bad).is_empty(), "{status} passed");
    }
    let mut self_approved = approvals.clone();
    self_approved[first]["resolvedBy"] = self_approved[first]["requestedBy"].clone();
    assert!(!problems(&spec, &state, &self_approved).is_empty());
    let mut wrong_item = approvals.clone();
    wrong_item[first]["subject"]["description"] = json!("gate:another-item:review");
    assert!(!problems(&spec, &state, &wrong_item).is_empty());
    for field in ["runId", "nodeId", "requestedBy", "resolvedBy"] {
        let mut missing_identity = approvals.clone();
        missing_identity[first]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!problems(&spec, &state, &missing_identity).is_empty());
        let mut wrong_identity = approvals.clone();
        wrong_identity[first][field] = json!("wrong");
        if field != "resolvedBy" {
            assert!(!problems(&spec, &state, &wrong_identity).is_empty());
        }
    }
    let mut partial = state.clone();
    partial["channels"]["__approvalIds"] = json!(["old-approved-id"]);
    let old = json!({"old-approved-id":{"status":"approved", "requestedBy":"old:review",
            "resolvedBy":"alice", "subject":{"description":"gate:old:review"}}});
    assert!(!problems(&spec, &partial, &old).is_empty());
    assert_eq!(host.0.load(Ordering::SeqCst), 0);
    spec["state"] = state;
    let done: Value = serde_json::from_str(
        &run(spec.to_string(), host.clone(), Entry::Resume)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(done["state"]["status"], "completed");
    assert_eq!(host.0.load(Ordering::SeqCst), if mapped { 2 } else { 1 });
}

#[tokio::test]
async fn nested_gate_requires_its_own_approval() {
    assert_governed_resume(false, true, "suspended").await;
}

#[tokio::test]
async fn every_map_item_requires_its_own_approval() {
    assert_governed_resume(true, false, "suspended").await;
}

#[tokio::test]
async fn subgraphs_inside_map_items_require_their_own_approvals() {
    assert_governed_resume(true, true, "suspended").await;
}

#[tokio::test]
async fn fully_approved_children_resume_from_cancelled_and_running_roots() {
    for status in ["cancelled", "running"] {
        assert_governed_resume(true, true, status).await;
    }
}

fn approved_records(spec: &Value, state: &Value) -> Value {
    let mut records = json!({});
    for (index, request) in plan(spec, state).requests.iter().enumerate() {
        records[format!("id-{index}")] = json!({
            "runId":request.run_id, "nodeId":request.node_id, "subject":request.subject,
            "requestedBy":request.requested_by, "status":"approved", "resolvedBy":"alice"
        });
    }
    records
}

#[test]
fn an_empty_resolver_cannot_approve_a_descendant_gate() {
    let spec = fixture(true, false);
    let mut state = map_wait(1);
    let mut approvals = approved_records(&spec, &state);
    state["channels"]["__approvalIds"] = json!(["id-0"]);
    assert!(problems(&spec, &state, &approvals).is_empty());
    approvals["id-0"]["resolvedBy"] = json!("");
    assert!(!problems(&spec, &state, &approvals).is_empty());
}

#[test]
fn an_approved_record_outside_the_stash_does_not_cover_an_item() {
    let spec = fixture(true, false);
    let mut state = map_wait(2);
    let approvals = approved_records(&spec, &state);
    state["channels"]["__approvalIds"] = json!(["id-0", "id-1"]);
    assert!(problems(&spec, &state, &approvals).is_empty());
    state["channels"]["__approvalIds"] = json!(["id-0"]);
    // Both records are approved and readable; only the first id is actually filed in this state.
    assert_eq!(approvals["id-1"]["status"], "approved");
    assert!(!problems(&spec, &state, &approvals).is_empty());
}

#[test]
fn approval_wait_limits_remain_the_accepted_sixteen_levels_and_256_requests() {
    use ailu_runtime_bridge::catalog_approvals::{MAX_FILED_REQUESTS, MAX_WAIT_DEPTH};
    assert_eq!(MAX_WAIT_DEPTH, 16);
    assert_eq!(MAX_FILED_REQUESTS, 256);
}

#[test]
fn legacy_descendant_wait_explains_recovery_and_a_fresh_plan_contains_every_request() {
    let spec = fixture(true, true);
    let mut state = map_wait(2);
    for index in 0..2 {
        let child = format!("root:sub:{index}");
        let inner = format!("{child}:inner");
        let mut states = serde_json::Map::new();
        states.insert(inner.clone(), snapshot(&inner, "review", json!({})));
        state["channels"]["__subgraphStates"][&child] =
            snapshot(&child, "inner", json!({"__subgraphStates":states}));
    }
    let error = problems(&spec, &state, &json!({}));
    assert_eq!(error.len(), 1);
    assert!(error[0].contains("descendant approval that was never recorded"));
    assert!(error[0].contains("recover its approval requests"));
    assert!(error[0].contains("or abandon the run"));

    // Recovery plans the saved checkpoint without executing it. A stale stash must be replaced
    // as a whole: retaining it makes filing idempotently return no additional requests.
    state["channels"]["__approvalIds"] = json!(["old-partial-id"]);
    assert!(plan(&spec, &state).requests.is_empty());
    state["channels"]["__approvalIds"] = json!([]);
    let filing = plan(&spec, &state);
    assert_eq!(filing.refusal, None);
    assert_eq!(filing.requests.len(), 2);
    assert_eq!(filing.requests[0].run_id, "root:sub:0:inner");
    assert_eq!(filing.requests[1].run_id, "root:sub:1:inner");
    let approvals = approved_records(&spec, &state);
    state["channels"]["__approvalIds"] = json!(["id-0", "id-1"]);
    assert!(problems(&spec, &state, &approvals).is_empty());
}

fn snapshot(run: &str, current: &str, channels: Value) -> Value {
    json!({"runId":run, "graphId":"leaf", "currentNodeId":current, "channels":channels,
        "status":"suspended", "version":1, "createdAt":"0", "updatedAt":"0"})
}

fn map_wait(count: usize) -> Value {
    let snapshots: serde_json::Map<String, Value> = (0..count)
        .map(|index| {
            let run = format!("root:sub:{index}");
            (run.clone(), snapshot(&run, "review", json!({})))
        })
        .collect();
    snapshot(
        "root",
        "sub",
        json!({"items":vec![json!({}); count], "__subgraphStates":snapshots}),
    )
}

fn assert_refused(spec: &Value, state: &Value) {
    let filing = plan(spec, state);
    assert!(filing.requests.is_empty());
    let refusal = filing
        .refusal
        .expect("an uninterpretable wait must be refused");
    assert_eq!(problems(spec, state, &json!({})), vec![refusal]);
}

#[test]
fn malformed_descendant_state_is_never_treated_as_an_empty_wait() {
    let spec = fixture(true, false);
    let state = map_wait(2);
    for field in ["channels", "currentNodeId", "runId", "status"] {
        let mut broken = state.clone();
        broken["channels"]["__subgraphStates"]["root:sub:0"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_refused(&spec, &broken);
    }
    for field in ["channels", "currentNodeId", "runId", "status"] {
        let mut broken = state.clone();
        broken["channels"]["__subgraphStates"]["root:sub:0"][field] = json!("unknown");
        assert_refused(&spec, &broken);
    }
    for bad in [Value::Null, json!("snapshot"), json!([])] {
        let mut broken = state.clone();
        broken["channels"]["__subgraphStates"]["root:sub:0"] = bad.clone();
        assert_refused(&spec, &broken);
        broken["channels"]["__subgraphStates"] = bad;
        assert_refused(&spec, &broken);
    }
    for key in [
        "root:sub:00",
        "root:sub:+0",
        "root:sub:-1",
        "root:sub:999999999999999999999",
        "unrelated",
    ] {
        let mut broken = state.clone();
        broken["channels"]["__subgraphStates"][key] = snapshot(key, "review", json!({}));
        assert_refused(&spec, &broken);
    }
    let mut shortened = state.clone();
    shortened["channels"]["items"] = json!([{}]);
    assert_refused(&spec, &shortened);
    let mut missing = spec.clone();
    missing["subgraphs"] = json!([]);
    assert_refused(&missing, &state);
    let mut missing_snapshot = state.clone();
    missing_snapshot["channels"]["__subgraphStates"] = json!({});
    assert_refused(&spec, &missing_snapshot);
}

#[test]
fn map_items_are_filed_in_numeric_order_and_the_request_bound_is_atomic() {
    use ailu_runtime_bridge::catalog_approvals::MAX_FILED_REQUESTS;
    let spec = fixture(true, false);
    let filing = plan(&spec, &map_wait(MAX_FILED_REQUESTS));
    assert_eq!(filing.refusal, None);
    assert_eq!(filing.requests.len(), MAX_FILED_REQUESTS);
    for (index, request) in filing.requests.iter().enumerate() {
        assert_eq!(request.run_id, format!("root:sub:{index}"));
    }
    assert_refused(&spec, &map_wait(MAX_FILED_REQUESTS + 1));
}

#[test]
fn depth_limit_refuses_the_whole_wait_and_repeated_run_ids_are_refused() {
    use ailu_runtime_bridge::catalog_approvals::MAX_WAIT_DEPTH;
    let mut spec = fixture(false, false);
    let chain = graph(
        "chain",
        json!([
            {"id":"sub", "type":"subgraph", "label":"sub", "subgraphId":"chain"},
            {"id":"review", "type":"human-gate", "label":"review"}
        ]),
        json!([]),
        "sub",
    );
    spec["graph"] = chain.clone();
    spec["subgraphs"] = json!([chain]);
    let tree = |depth: usize| {
        let runs: Vec<_> = (0..=depth)
            .map(|index| format!("root{}", ":sub".repeat(index)))
            .collect();
        let mut state = snapshot(&runs[depth], "review", json!({}));
        for index in (0..depth).rev() {
            let mut states = serde_json::Map::new();
            states.insert(runs[index + 1].clone(), state);
            state = snapshot(&runs[index], "sub", json!({"__subgraphStates":states}));
        }
        state
    };
    assert_eq!(plan(&spec, &tree(MAX_WAIT_DEPTH)).requests.len(), 1);
    assert_refused(&spec, &tree(MAX_WAIT_DEPTH + 1));
    let repeated = snapshot(
        "root",
        "sub",
        json!({"__subgraphRuns":{"sub":"root"},
        "__subgraphStates":{"root":snapshot("root", "review", json!({}))}}),
    );
    assert_refused(&spec, &repeated);
}

#[test]
fn replay_aliases_and_completed_siblings_preserve_the_outstanding_wait() {
    let spec = fixture(true, false);
    let mut state = map_wait(2);
    // An already completed item has only its kept join result; it needs no new signature.
    state["channels"]["__subgraphStates"]
        .as_object_mut()
        .unwrap()
        .remove("root:sub:0");
    state["channels"]["__mapResults"] = json!({"root:sub:0":{"itemHash":"kept", "result":{}}});
    state["runId"] = json!("root:fork:7");
    let filing = plan(&spec, &state);
    assert_eq!(filing.refusal, None);
    assert_eq!(filing.requests.len(), 1);
    assert_eq!(filing.requests[0].run_id, "root:sub:1");
    let mut aliases = state.clone();
    aliases["channels"]["__subgraphStates"]["root:fork:7:sub:1"] =
        snapshot("root:fork:7:sub:1", "review", json!({}));
    assert_refused(&spec, &aliases);
}

#[test]
fn lifecycle_copies_and_partial_stashes_cannot_bypass_suspended_siblings() {
    let spec = fixture(true, false);
    for status in ["suspended", "cancelled", "running", "completed", "failed"] {
        let mut state = map_wait(2);
        state["status"] = json!(status);
        let filing = plan(&spec, &state);
        assert_eq!(filing.requests.len(), 2, "{status}");
        assert!(!problems(&spec, &state, &json!({})).is_empty());
        state["channels"]["__approvalIds"] = json!(["only-first"]);
        let approvals = json!({"only-first":{"status":"approved", "resolvedBy":"alice",
            "runId":filing.requests[0].run_id, "nodeId":filing.requests[0].node_id,
            "requestedBy":filing.requests[0].requested_by, "subject":filing.requests[0].subject}});
        assert!(!problems(&spec, &state, &approvals).is_empty(), "{status}");
    }
}

#[test]
fn malformed_descendant_tool_requests_are_refused_instead_of_ignored() {
    let mut spec = fixture(true, false);
    spec["subgraphs"][1]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"agent", "type":"agent", "label":"agent",
            "metadata":{"agent":{"outputChannel":"agentResult", "toolNames":["refund"]}}
        }));
    for requests in [Value::Null, json!({}), json!([{}]), json!([{"subject":42}])] {
        let mut state = map_wait(1);
        state["channels"]["__subgraphStates"]["root:sub:0"]["channels"]["agentResult"] =
            json!({"approvalRequests":requests});
        assert_refused(&spec, &state);
    }
}

#[test]
fn an_unreached_map_does_not_validate_input_a_prior_step_will_prepare() {
    let mut spec = fixture(true, false);
    spec["graph"]["nodes"].as_array_mut().unwrap().push(json!({
        "id":"review", "type":"human-gate", "label":"review"
    }));
    let state = snapshot(
        "root",
        "review",
        json!({"items":"raw input to normalize later"}),
    );
    let filing = plan(&spec, &state);
    assert_eq!(filing.refusal, None);
    assert_eq!(filing.requests.len(), 1);
    assert_eq!(filing.requests[0].subject.description, "gate:review");
}

#[test]
fn returning_to_a_deep_gate_files_a_new_decision_and_redriving_does_not_duplicate_it() {
    let spec = fixture(true, false);
    let mut state = map_wait(2);
    state["channels"]["__approvalIds"] = json!(["old-0", "old-1"]);
    assert!(plan(&spec, &state).requests.is_empty());
    let input: FilingInput = serde_json::from_value(json!({
        "graph":spec["graph"], "subgraphs":spec["subgraphs"],
        "previousState":state, "state":state
    }))
    .unwrap();
    let next = filing_plan(&input);
    assert!(next.clear_approval_ids);
    assert_eq!(next.requests.len(), 2);
}
