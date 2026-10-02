//! Approvals of a catalog run, decided by the engine (ADR 0045 D3.1).
//!
//! A governed catalog run — its host keeps an approval store — waits on a person in two ways: an
//! agent asks to use a gated tool (its output channel lists `approvalRequests`), or the run stops
//! at a human gate. The engine decides what the host files and whether a resume may go on; the
//! host only stores the requests and reads them back:
//!
//! 1. after a run or a resume, [`filing_plan`] lists the requests to file: one per gated tool an
//!    agent asked for (subject `tool:<name>`), one for the human gate the run waits at (subject
//!    `gate:<node id>`), and the same for a direct child run suspended inside a subgraph node —
//!    filed under the child's run id, its node ids prefixed with it. The host files them and
//!    stashes their ids in the state's `__approvalIds` channel. A resume that now waits on
//!    something else clears the ids stashed for the previous wait first;
//! 2. before a resume, [`approvals_to_check`] names the stashed requests to read back, and
//!    [`resume_problems`] says why the resume may not go on: a request the store does not know or
//!    that is still pending, a rejected gate, a request approved by its own requester, a granted
//!    tool that no request approved by that same approver, or a wait that was never filed.
//!
//! These are the rules the TypeScript SDK applied itself before (`fileApprovalRequests`,
//! `ensureApprovalsGranted`), with the same wording for the problems. Scope, unchanged: a nested
//! subgraph inside a child, and a `mapSubgraph` fan-out's children, are not walked.

use ailu_agents_core::DEFAULT_AGENT_OUTPUT_CHANNEL;
use ailu_graph_runtime::{SUBGRAPH_RUNS_KEY, SUBGRAPH_STATES_KEY};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::catalog::read_agent_carrier;
use crate::spec::ApprovedTool;

/// The channel a governed run's filed request ids are stashed in.
pub const APPROVAL_IDS_CHANNEL: &str = "__approvalIds";
/// Subject prefix of a gated tool's request: `tool:<name>`.
pub const TOOL_SUBJECT_PREFIX: &str = "tool:";
/// Subject prefix of a human gate's request: `gate:<node id>` — told apart from a tool's, since a
/// rejected gate blocks the resume while a rejected tool only stays locked.
pub const GATE_SUBJECT_PREFIX: &str = "gate:";

/// What a request is about: `{ "description": "tool:<name>" | "gate:<node id>" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSubject {
    pub description: String,
}

/// A request the host files in its approval store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalToFile {
    /// The run the request belongs to: the run's own id, or a child run's for a child's request.
    pub run_id: String,
    /// The node that asked, prefixed with `<child run id>:` for a child's node.
    pub node_id: String,
    /// The requester — the same as `node_id`: a person other than it resolves the request.
    pub requested_by: String,
    pub subject: ApprovalSubject,
}

/// A catalog run's state after a run or a resume, and the one before a resume.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilingInput {
    /// The catalog `GraphDefinition`.
    pub graph: Value,
    #[serde(default)]
    pub subgraphs: Option<Vec<Value>>,
    /// The `GraphState` the run returned.
    pub state: Value,
    /// For a resume: the state it resumed from.
    #[serde(default)]
    pub previous_state: Option<Value>,
}

/// What the host does with a run's state before keeping it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilingPlan {
    /// Set `__approvalIds` to `[]` first: the resumed run waits on something else than the ids
    /// stashed for its previous wait.
    pub clear_approval_ids: bool,
    /// The requests to file, in order; their ids then go to `__approvalIds`. Empty when the run is
    /// not suspended, waits on nothing a person decides, or already stashed its ids.
    pub requests: Vec<ApprovalToFile>,
}

/// A resume the host is about to make, and what its store holds for the stashed ids.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeCheckInput {
    pub graph: Value,
    #[serde(default)]
    pub subgraphs: Option<Vec<Value>>,
    /// The suspended `GraphState` to resume.
    pub state: Value,
    /// The tools the resume grants.
    #[serde(default)]
    pub approved_tools: Vec<ApprovedTool>,
    /// The store's record for each id [`approvals_to_check`] named — `{ status, subject,
    /// requestedBy, resolvedBy }`, as the store returns it — or `null` (or no entry) when the store
    /// does not know the id.
    #[serde(default)]
    pub approvals: Map<String, Value>,
}

/// A stored request, as far as the decision reads it.
struct StoredApproval<'a> {
    /// `"pending" | "approved" | "rejected"`.
    status: &'a str,
    /// The subject's `description`; empty for another subject (an artifact reference).
    subject: &'a str,
    requested_by: Option<&'a str>,
    resolved_by: Option<&'a str>,
}

impl<'a> StoredApproval<'a> {
    fn read(record: &'a Value) -> Option<Self> {
        let record = record.as_object()?;
        let text = |key: &str| record.get(key).and_then(Value::as_str);
        Some(Self {
            status: text("status").unwrap_or_default(),
            subject: record
                .get("subject")
                .and_then(|subject| subject.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            requested_by: text("requestedBy"),
            resolved_by: text("resolvedBy"),
        })
    }
}

fn is_suspended(state: &Value) -> bool {
    state.get("status").and_then(Value::as_str) == Some("suspended")
}

fn channels_of(state: &Value) -> Option<&Map<String, Value>> {
    state.get("channels").and_then(Value::as_object)
}

/// The nodes of a definition (none when it has no node list).
fn nodes_of(graph: &Value) -> &[Value] {
    graph
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn node_id(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

/// An approval request's subject as `{ description }`: a string subject, or an object with a
/// string `description`. Anything else is not a request.
fn normalize_subject(request: &Value) -> Option<ApprovalSubject> {
    let subject = request.as_object()?.get("subject")?;
    let description = match subject {
        Value::String(description) => description,
        Value::Object(subject) => subject.get("description")?.as_str()?,
        _ => return None,
    };
    Some(ApprovalSubject {
        description: description.to_owned(),
    })
}

/// The approval requests an agent's output channel lists.
fn approval_requests(channels: &Map<String, Value>, output_channel: &str) -> Vec<ApprovalSubject> {
    channels
        .get(output_channel)
        .and_then(|channel| channel.get("approvalRequests"))
        .and_then(Value::as_array)
        .map(|requests| requests.iter().filter_map(normalize_subject).collect())
        .unwrap_or_default()
}

/// One request per gated tool the agents of `nodes` asked for, read from `channels` (a run's, or
/// a child's nested snapshot), filed under `run_id`.
fn agent_requests(
    nodes: &[Value],
    channels: &Map<String, Value>,
    run_id: &str,
    id_prefix: &str,
) -> Vec<ApprovalToFile> {
    let mut requests = Vec::new();
    for node in nodes {
        let (Some(id), Some(agent)) = (node_id(node), read_agent_carrier(node.get("metadata")))
        else {
            continue;
        };
        let output_channel = match agent.get("outputChannel") {
            None | Some(Value::Null) => DEFAULT_AGENT_OUTPUT_CHANNEL,
            Some(Value::String(channel)) => channel.as_str(),
            // The engine refuses such a spec: the run never happened.
            Some(_) => continue,
        };
        for subject in approval_requests(channels, output_channel) {
            requests.push(ApprovalToFile {
                run_id: run_id.to_owned(),
                node_id: format!("{id_prefix}{id}"),
                requested_by: format!("{id_prefix}{id}"),
                subject,
            });
        }
    }
    requests
}

/// One request for the human gate a run (or a child) is suspended at, if it is suspended at one.
fn gate_request(
    nodes: &[Value],
    current_node_id: Option<&Value>,
    run_id: &str,
    id_prefix: &str,
) -> Option<ApprovalToFile> {
    let current = current_node_id?.as_str()?;
    let gate = nodes.iter().find(|node| {
        node_id(node) == Some(current)
            && node.get("type").and_then(Value::as_str) == Some("human-gate")
    })?;
    let id = node_id(gate)?;
    Some(ApprovalToFile {
        run_id: run_id.to_owned(),
        node_id: format!("{id_prefix}{id}"),
        requested_by: format!("{id_prefix}{id}"),
        subject: ApprovalSubject {
            description: format!("{GATE_SUBJECT_PREFIX}{id_prefix}{id}"),
        },
    })
}

/// A subgraph node's child run id: the one the engine recorded in `__subgraphRuns`, else the
/// deterministic `<run id>:<node id>` it uses for a child that has not recorded one yet.
fn child_run_id(channels: &Map<String, Value>, run_id: &str, node_id: &str) -> String {
    channels
        .get(SUBGRAPH_RUNS_KEY)
        .and_then(|runs| runs.get(node_id))
        .and_then(Value::as_str)
        .map_or_else(|| format!("{run_id}:{node_id}"), str::to_owned)
}

/// Every request a suspended state waits on, ignoring what is already stashed.
fn requests_of(graph: &Value, subgraphs: &[Value], state: &Value) -> Vec<ApprovalToFile> {
    let Some(channels) = channels_of(state) else {
        return Vec::new();
    };
    let run_id = state
        .get("runId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let nodes = nodes_of(graph);
    let mut requests = agent_requests(nodes, channels, run_id, "");
    requests.extend(gate_request(nodes, state.get("currentNodeId"), run_id, ""));

    for node in nodes {
        // A direct child only: one subgraph reference, not a `mapSubgraph` fan-out (no single
        // child id to file under).
        if node.get("type").and_then(Value::as_str) != Some("subgraph")
            || node.get("mapSubgraph").is_some()
        {
            continue;
        }
        let (Some(id), Some(subgraph_id)) = (
            node_id(node),
            node.get("subgraphId").and_then(Value::as_str),
        ) else {
            continue;
        };
        // A later definition with the same id wins, as in a map keyed by graph id.
        let Some(child) = subgraphs
            .iter()
            .rev()
            .find(|graph| graph.get("id").and_then(Value::as_str) == Some(subgraph_id))
        else {
            continue;
        };
        let child_run = child_run_id(channels, run_id, id);
        let Some(snapshot) = channels
            .get(SUBGRAPH_STATES_KEY)
            .and_then(|states| states.get(&child_run))
            .filter(|snapshot| snapshot.is_object())
        else {
            continue;
        };
        let Some(child_channels) = snapshot.get("channels").and_then(Value::as_object) else {
            continue;
        };
        if !is_suspended(snapshot) {
            continue;
        }
        let prefix = format!("{child_run}:");
        let child_nodes = nodes_of(child);
        requests.extend(agent_requests(
            child_nodes,
            child_channels,
            &child_run,
            &prefix,
        ));
        requests.extend(gate_request(
            child_nodes,
            snapshot.get("currentNodeId"),
            &child_run,
            &prefix,
        ));
    }
    requests
}

/// What a suspended state waits on: its node and the subjects of the approval requests its
/// channels list (sorted). Empty for a state that is not suspended.
fn suspension_key(state: &Value) -> Option<(Option<&Value>, Vec<String>)> {
    if !is_suspended(state) {
        return None;
    }
    let mut subjects: Vec<String> = channels_of(state)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(|value| value.get("approvalRequests").and_then(Value::as_array))
        .flatten()
        .map(|request| request.get("subject").unwrap_or(&Value::Null).to_string())
        .collect();
    subjects.sort();
    Some((state.get("currentNodeId"), subjects))
}

/// Whether the state already stashed the ids of the requests it waits on.
fn has_stashed_ids(state: &Value) -> bool {
    channels_of(state)
        .and_then(|channels| channels.get(APPROVAL_IDS_CHANNEL))
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty())
}

/// What the host files for the state a run or a resume returned (see the module docs).
#[must_use]
pub fn filing_plan(input: &FilingInput) -> FilingPlan {
    let clear_approval_ids = input
        .previous_state
        .as_ref()
        .is_some_and(|previous| suspension_key(previous) != suspension_key(&input.state));
    let stashed = has_stashed_ids(&input.state) && !clear_approval_ids;
    let requests = if is_suspended(&input.state) && !stashed {
        requests_of(
            &input.graph,
            input.subgraphs.as_deref().unwrap_or_default(),
            &input.state,
        )
    } else {
        Vec::new()
    };
    FilingPlan {
        clear_approval_ids,
        requests,
    }
}

/// The stashed request ids a resume of `state` must read back from the store: none when the run
/// is not suspended.
#[must_use]
pub fn approvals_to_check(state: &Value) -> Vec<String> {
    if !is_suspended(state) {
        return Vec::new();
    }
    channels_of(state)
        .and_then(|channels| channels.get(APPROVAL_IDS_CHANNEL))
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .map(|id| match id {
                    Value::String(id) => id.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Why a resume of `input.state` may not go on — empty when it may (see the module docs).
#[must_use]
pub fn resume_problems(input: &ResumeCheckInput) -> Vec<String> {
    if !is_suspended(&input.state) {
        return Vec::new();
    }
    let ids = approvals_to_check(&input.state);
    if ids.is_empty() {
        let waits = requests_of(
            &input.graph,
            input.subgraphs.as_deref().unwrap_or_default(),
            &input.state,
        );
        return if waits.is_empty() {
            Vec::new()
        } else {
            vec!["the run waits on an approval that was never recorded: start it with the same approvalEngine".to_owned()]
        };
    }

    let mut problems = Vec::new();
    let mut approved_tools: Vec<StoredApproval> = Vec::new();
    for id in &ids {
        let Some(record) = input.approvals.get(id).and_then(StoredApproval::read) else {
            problems.push(format!("request {id} is unknown to the approval engine"));
            continue;
        };
        let subject = record.subject;
        match record.status {
            "pending" => problems.push(format!("request {id} ({subject}) is still pending")),
            "rejected" if subject.starts_with(GATE_SUBJECT_PREFIX) => problems.push(format!(
                "request {id} ({subject}) was rejected by {}",
                record.resolved_by.unwrap_or("a reviewer")
            )),
            // A store must refuse it; the engine does not take its word for it.
            "approved"
                if record.resolved_by.is_some() && record.resolved_by == record.requested_by =>
            {
                problems.push(format!(
                    "request {id} ({subject}) was approved by its own requester"
                ));
            }
            "approved" if subject.starts_with(TOOL_SUBJECT_PREFIX) => approved_tools.push(record),
            _ => {}
        }
    }
    for grant in &input.approved_tools {
        let wanted = format!("{TOOL_SUBJECT_PREFIX}{}", grant.name);
        let matched = approved_tools.iter().any(|record| {
            record.subject == wanted && record.resolved_by == Some(grant.resolved_by.as_str())
        });
        if !matched {
            problems.push(format!(
                "tool '{}' has no request approved by '{}' in the approval engine",
                grant.name, grant.resolved_by
            ));
        }
    }
    problems
}

/// [`filing_plan`] for the bindings: a [`FilingInput`] as JSON in, the [`FilingPlan`] as JSON out.
///
/// # Errors
///
/// When the input is not a JSON [`FilingInput`].
pub fn filing_plan_json(input_json: &str) -> Result<String, String> {
    let input: FilingInput = serde_json::from_str(input_json)
        .map_err(|error| format!("invalid approval filing input JSON: {error}"))?;
    serde_json::to_string(&filing_plan(&input)).map_err(|error| error.to_string())
}

/// [`approvals_to_check`] for the bindings: a `GraphState` as JSON in, the ids as a JSON array out.
///
/// # Errors
///
/// When the state is not JSON.
pub fn approvals_to_check_json(state_json: &str) -> Result<String, String> {
    let state: Value =
        serde_json::from_str(state_json).map_err(|error| format!("invalid state JSON: {error}"))?;
    Ok(json!(approvals_to_check(&state)).to_string())
}

/// [`resume_problems`] for the bindings: a [`ResumeCheckInput`] as JSON in, the problems as a JSON
/// array out.
///
/// # Errors
///
/// When the input is not a JSON [`ResumeCheckInput`].
pub fn resume_problems_json(input_json: &str) -> Result<String, String> {
    let input: ResumeCheckInput = serde_json::from_str(input_json)
        .map_err(|error| format!("invalid resume check input JSON: {error}"))?;
    Ok(json!(resume_problems(&input)).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(nodes: Value) -> Value {
        json!({ "id": "g", "version": "1", "name": "g", "channels": {}, "nodes": nodes,
                "edges": [], "entryNodeId": "assistant" })
    }

    fn suspended(current: &str, channels: Value) -> Value {
        json!({ "runId": "run-1", "graphId": "g", "currentNodeId": current, "status": "suspended",
                "channels": channels, "version": 1, "createdAt": "0", "updatedAt": "0" })
    }

    fn gated_agent() -> Value {
        graph(json!([
            { "id": "assistant", "type": "agent", "label": "assistant",
              "metadata": { "agent": { "toolNames": ["refund"], "suspendForApproval": true } } },
            { "id": "review", "type": "human-gate", "label": "review" }
        ]))
    }

    fn check(state: Value, approvals: Value, grants: Value) -> Vec<String> {
        resume_problems(
            &serde_json::from_value(json!({
                "graph": gated_agent(), "state": state, "approvals": approvals,
                "approvedTools": grants
            }))
            .expect("check input parses"),
        )
    }

    #[test]
    fn a_suspended_run_files_its_tools_then_its_gate() {
        let plan = filing_plan(&FilingInput {
            graph: gated_agent(),
            subgraphs: None,
            state: suspended(
                "review",
                json!({ "agentResult": { "approvalRequests": [{ "subject": "tool:refund" }] } }),
            ),
            previous_state: None,
        });
        assert!(!plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| {
                (
                    request.node_id.as_str(),
                    request.subject.description.as_str(),
                )
            })
            .collect();
        assert_eq!(
            filed,
            vec![("assistant", "tool:refund"), ("review", "gate:review")]
        );
        assert!(plan
            .requests
            .iter()
            .all(|request| request.run_id == "run-1" && request.requested_by == request.node_id));
    }

    #[test]
    fn a_tool_approved_by_its_own_requester_is_refused_and_unlocks_nothing() {
        let state = suspended("assistant", json!({ "__approvalIds": ["a-1"] }));
        let approvals = json!({ "a-1": { "status": "approved", "subject": { "description": "tool:refund" },
                                         "requestedBy": "assistant", "resolvedBy": "assistant" } });
        let grants =
            json!([{ "name": "refund", "requestedBy": "assistant", "resolvedBy": "assistant" }]);
        assert_eq!(
            check(state, approvals, grants),
            vec![
                "request a-1 (tool:refund) was approved by its own requester".to_owned(),
                "tool 'refund' has no request approved by 'assistant' in the approval engine"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn a_granted_tool_needs_the_request_approved_by_the_same_approver() {
        let state = suspended("assistant", json!({ "__approvalIds": ["a-1"] }));
        let approvals = json!({ "a-1": { "status": "approved", "subject": { "description": "tool:refund" },
                                         "requestedBy": "assistant", "resolvedBy": "alice" } });
        let grant =
            |by: &str| json!([{ "name": "refund", "requestedBy": "assistant", "resolvedBy": by }]);
        assert!(check(state.clone(), approvals.clone(), grant("alice")).is_empty());
        assert_eq!(check(state, approvals, grant("bob")).len(), 1);
    }

    #[test]
    fn only_a_suspended_run_has_approvals_to_check() {
        let mut state = suspended("review", json!({ "__approvalIds": ["a-1", 2] }));
        assert_eq!(
            approvals_to_check(&state),
            vec!["a-1".to_owned(), "2".to_owned()]
        );
        state["status"] = json!("completed");
        assert!(approvals_to_check(&state).is_empty());
    }

    #[test]
    fn the_json_entries_refuse_input_that_does_not_parse() {
        assert!(filing_plan_json("{").is_err());
        assert!(resume_problems_json("{\"graph\": {}}").is_err());
        assert!(approvals_to_check_json("not json").is_err());
        assert_eq!(approvals_to_check_json("{}").unwrap(), "[]");
    }
}
