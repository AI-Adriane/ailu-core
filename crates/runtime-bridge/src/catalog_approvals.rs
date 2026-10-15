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
//!    stashes their ids in the state's `__approvalIds` channel — unless the plan carries a
//!    `refusal`: a wait no person can decide (a tool approval in a child run, which no grant can
//!    reach yet), filed nowhere, for which the host fails the run. A resume that now waits on
//!    something else clears the ids stashed for the previous wait first and files the new one —
//!    another call of the same tool at the same node is something else (ADR 0046), and so is a
//!    human gate the resume passed and a loop came back to: each visit is decided again; and so is
//!    a call whose grant the resume gave back, asked for again once it ran on it (ADR 0051 R1);
//! 2. before a resume, [`approvals_to_check`] names the stashed requests to read back, and
//!    [`resume_problems`] says why the resume may not go on: a request the store does not know or
//!    that is still pending, a rejected gate, a request approved by its own requester, a granted
//!    tool that no request approved by that same approver, a wait that was never filed — or, first
//!    and alone, the plan's refusal.
//!
//! These are the rules the TypeScript SDK applied itself before (`fileApprovalRequests`,
//! `ensureApprovalsGranted`), with the same wording for the problems. Scope, unchanged: a nested
//! subgraph inside a child, and a `mapSubgraph` fan-out's children, are not walked.

use ailu_agents_core::{APPROVED_TOOLS_CHANNEL, DEFAULT_AGENT_OUTPUT_CHANNEL};
use ailu_graph_runtime::{SUBGRAPH_RUNS_KEY, SUBGRAPH_STATES_KEY};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::catalog::read_agent_carrier;

use crate::spec::ApprovedTool;
/// The canonical form of a call (ADR 0051 D1), for the bindings: see
/// [`ailu_agents_core::call_key_of`].
pub use ailu_agents_core::{call_input_of_json, call_key_of_json};

/// The channel a governed run's filed request ids are stashed in.
pub const APPROVAL_IDS_CHANNEL: &str = "__approvalIds";
/// Subject prefix of a gated tool's request: `tool:<name>`.
pub const TOOL_SUBJECT_PREFIX: &str = "tool:";
/// Subject prefix of a human gate's request: `gate:<node id>` — told apart from a tool's, since a
/// rejected gate blocks the resume while a rejected tool only stays locked.
pub const GATE_SUBJECT_PREFIX: &str = "gate:";

/// Why a run that waits on a tool approval in a child run is refused (ADR 0045 rev. 1, R6): a grant
/// cannot reach a child run yet — the bridge writes `__approvedTools` into the top-level state only,
/// and a child resumes from its own snapshot — so filing it would loop: approved, asked again, on
/// every resume. The host fails the run with this reason instead.
pub const CHILD_TOOL_GRANT_REFUSAL: &str =
    "a tool approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6)";

/// What a request is about: `{ "description": "tool:<name>" | "gate:<node id>" }` — and, for a
/// call gated by its own content (a guarded write, ADR 0024; a threshold crossed, ADR 0046), the
/// grant key the host gives back on resume, the call's input it shows the signer, and what
/// crossed. A request filed before ADR 0046 has none of them, and resumes as before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSubject {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl ApprovalSubject {
    /// A subject that is only a description (a human gate).
    fn described(description: String) -> Self {
        Self {
            description,
            approval_key: None,
            input: None,
            condition: None,
        }
    }
}

/// A request the host files in its approval store.
#[derive(Clone, Debug, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilingPlan {
    /// Set `__approvalIds` to `[]` first: the resumed run waits on something else than the ids
    /// stashed for its previous wait — another call of the same tool included.
    pub clear_approval_ids: bool,
    /// The requests to file, in order; their ids then go to `__approvalIds`. Empty when the run is
    /// not suspended, waits on nothing a person decides, already stashed its ids, or is refused.
    pub requests: Vec<ApprovalToFile>,
    /// Why the run waits on something no person can decide (ADR 0045 rev. 1, R6): nothing is
    /// filed, a resume is refused with this reason, and the host fails the run with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
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
    /// The grant key filed with the request (ADR 0046 D4), when the call was gated by its content.
    approval_key: Option<&'a str>,
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
            approval_key: record
                .get("subject")
                .and_then(|subject| subject.get("approvalKey"))
                .and_then(Value::as_str),
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
/// string `description`. Anything else is not a request. The request's `approvalKey`, `input` and
/// `condition` (ADR 0046 D4) are filed with it when present.
fn normalize_subject(request: &Value) -> Option<ApprovalSubject> {
    let request = request.as_object()?;
    let subject = request.get("subject")?;
    let description = match subject {
        Value::String(description) => description,
        Value::Object(subject) => subject.get("description")?.as_str()?,
        _ => return None,
    };
    Some(ApprovalSubject {
        description: description.to_owned(),
        approval_key: request
            .get("approvalKey")
            .and_then(Value::as_str)
            .map(str::to_owned),
        input: request
            .get("input")
            .filter(|input| !input.is_null())
            .cloned(),
        condition: request
            .get("condition")
            .and_then(Value::as_str)
            .map(str::to_owned),
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
        subject: ApprovalSubject::described(format!("{GATE_SUBJECT_PREFIX}{id_prefix}{id}")),
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

/// What a suspended state waits on: its node, the subjects of the approval requests its channels
/// list, and the requests a host files for it — each in full, so two waits on the same tool at the
/// same node differ by the call they hold (its grant key and input, ADR 0046: `refund(A)` then
/// `refund(B)`), and a child that moved on to another gate inside the same subgraph node differs
/// by the child's request. Sorted: listing the same requests in another order is the same wait.
/// Empty for a state that is not suspended.
fn suspension_key<'a>(
    graph: &Value,
    subgraphs: &[Value],
    state: &'a Value,
) -> Option<(Option<&'a Value>, Vec<String>, Vec<String>)> {
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
    let mut filed: Vec<String> = requests_of(graph, subgraphs, state)
        .iter()
        .map(|request| json!(request).to_string())
        .collect();
    filed.sort();
    Some((state.get("currentNodeId"), subjects, filed))
}

/// Whether the state already stashed the ids of the requests it waits on.
fn has_stashed_ids(state: &Value) -> bool {
    channels_of(state)
        .and_then(|channels| channels.get(APPROVAL_IDS_CHANNEL))
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty())
}

/// Whether a suspended state waits on a call of its own whose grant it holds (ADR 0051 review R1):
/// the resume gave that grant back, the call ran on it, and the agent asks for the same call
/// again — a grant that is a call key is spent by its call (ADR 0051 D5). The request it waits on
/// is therefore a new one, even when it reads exactly like the previous wait's.
fn waits_on_a_granted_call(graph: &Value, subgraphs: &[Value], state: &Value) -> bool {
    let run_id = state
        .get("runId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let granted: Vec<&str> = channels_of(state)
        .and_then(|channels| channels.get(APPROVED_TOOLS_CHANNEL))
        .and_then(Value::as_array)
        .map(|grants| grants.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    !granted.is_empty()
        && requests_of(graph, subgraphs, state).iter().any(|request| {
            request.run_id == run_id
                && request
                    .subject
                    .approval_key
                    .as_deref()
                    .is_some_and(|key| granted.contains(&key))
        })
}

/// Whether a suspended state waits at a human gate — its own, or a direct child's.
fn waits_at_a_gate(graph: &Value, subgraphs: &[Value], state: &Value) -> bool {
    is_suspended(state)
        && requests_of(graph, subgraphs, state)
            .iter()
            .any(|request| request.subject.description.starts_with(GATE_SUBJECT_PREFIX))
}

/// Why no person can decide what a suspended state waits on, if so: a tool request of a child run
/// (ADR 0045 rev. 1, R6). A grant cannot reach a child run yet, so approving it would loop.
fn refusal_of(graph: &Value, subgraphs: &[Value], state: &Value) -> Option<String> {
    let run_id = state
        .get("runId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    requests_of(graph, subgraphs, state)
        .iter()
        .any(|request| {
            request.run_id != run_id && request.subject.description.starts_with(TOOL_SUBJECT_PREFIX)
        })
        .then(|| CHILD_TOOL_GRANT_REFUSAL.to_owned())
}

/// What the host files for the state a run or a resume returned (see the module docs).
#[must_use]
pub fn filing_plan(input: &FilingInput) -> FilingPlan {
    let subgraphs = input.subgraphs.as_deref().unwrap_or_default();
    let clear_approval_ids = input.previous_state.as_ref().is_some_and(|previous| {
        suspension_key(&input.graph, subgraphs, previous)
            != suspension_key(&input.graph, subgraphs, &input.state)
            // A resume passes the human gate it waited at — it always advances — so waiting at a
            // gate again is a new visit, brought back by a loop: a person decides it again.
            || (waits_at_a_gate(&input.graph, subgraphs, previous)
                && waits_at_a_gate(&input.graph, subgraphs, &input.state))
            // A wait on a call whose grant the resume gave back: that grant was spent, so the
            // call waits on a new decision (ADR 0051 review R1).
            || (is_suspended(&input.state)
                && waits_on_a_granted_call(&input.graph, subgraphs, &input.state))
    });
    let refusal = if is_suspended(&input.state) {
        refusal_of(&input.graph, subgraphs, &input.state)
    } else {
        None
    };
    let stashed = has_stashed_ids(&input.state) && !clear_approval_ids;
    let requests = if is_suspended(&input.state) && !stashed && refusal.is_none() {
        requests_of(&input.graph, subgraphs, &input.state)
    } else {
        Vec::new()
    };
    FilingPlan {
        clear_approval_ids,
        requests,
        refusal,
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
    let subgraphs = input.subgraphs.as_deref().unwrap_or_default();
    // A wait no person can decide: refused first, whatever was stashed before (R6).
    if let Some(refusal) = refusal_of(&input.graph, subgraphs, &input.state) {
        return vec![refusal];
    }
    let ids = approvals_to_check(&input.state);
    if ids.is_empty() {
        let waits = requests_of(&input.graph, subgraphs, &input.state);
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
        // A grant for one call (its key, ADR 0046) needs the request filed for THAT call: a key
        // the store never filed, or one of another request, unlocks nothing.
        let matched = approved_tools.iter().any(|record| {
            record.subject == wanted
                && record.resolved_by == Some(grant.resolved_by.as_str())
                && grant
                    .key
                    .as_deref()
                    .is_none_or(|key| record.approval_key == Some(key))
        });
        if !matched {
            problems.push(match &grant.key {
                Some(key) => format!(
                    "tool '{}' has no request for the call {key} approved by '{}' in the approval engine",
                    grant.name, grant.resolved_by
                ),
                None => format!(
                    "tool '{}' has no request approved by '{}' in the approval engine",
                    grant.name, grant.resolved_by
                ),
            });
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
    fn a_call_gated_by_its_content_is_filed_with_its_key_input_and_what_crossed() {
        // ADR 0046 D4: the host stores what the engine filed, gives the key back on resume, and
        // shows the signer the arguments; a request with none of them stays a bare description.
        let request = json!({
            "subject": "tool:refund",
            "reason": "Tool 'refund' requires human approval before execution: amount 600 > 500.",
            "approvalKey": format!("refund#{}", "a".repeat(64)),
            "input": { "amount": 600, "order": "A-2" },
            "condition": "amount 600 > 500"
        });
        let plan = filing_plan(&FilingInput {
            graph: gated_agent(),
            subgraphs: None,
            state: suspended(
                "assistant",
                json!({ "agentResult": { "approvalRequests": [request, { "subject": "tool:refund" }] } }),
            ),
            previous_state: None,
        });
        let subjects: Vec<Value> = plan
            .requests
            .iter()
            .map(|request| serde_json::to_value(&request.subject).expect("serializes"))
            .collect();
        assert_eq!(
            subjects,
            vec![
                json!({
                    "description": "tool:refund",
                    "approvalKey": format!("refund#{}", "a".repeat(64)),
                    "input": { "amount": 600, "order": "A-2" },
                    "condition": "amount 600 > 500"
                }),
                json!({ "description": "tool:refund" })
            ]
        );
    }

    #[test]
    fn a_grant_for_one_call_needs_the_request_filed_for_that_call() {
        let key = format!("refund#{}", "b".repeat(64));
        let state = suspended("assistant", json!({ "__approvalIds": ["a-1"] }));
        let approvals = json!({ "a-1": { "status": "approved",
            "subject": { "description": "tool:refund", "approvalKey": key },
            "requestedBy": "assistant", "resolvedBy": "alice" } });
        let grant = |key: &str| json!([{ "name": "refund", "requestedBy": "assistant", "resolvedBy": "alice", "key": key }]);
        // The key the store filed: accepted.
        assert!(check(state.clone(), approvals.clone(), grant(&key)).is_empty());
        // Another call's key: refused — a host cannot unlock a call nobody signed.
        let other = format!("refund#{}", "c".repeat(64));
        let problems = check(state.clone(), approvals.clone(), grant(&other));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains(&other));
        // A request filed before ADR 0046 carries no key: a keyed grant finds nothing to match.
        let before = json!({ "a-1": { "status": "approved", "subject": { "description": "tool:refund" },
                                      "requestedBy": "assistant", "resolvedBy": "alice" } });
        assert_eq!(check(state, before, grant(&key)).len(), 1);
    }

    /// The plan for a resume from `previous` that returned `state`.
    fn resumed(
        graph: Value,
        subgraphs: Option<Vec<Value>>,
        previous: Value,
        state: Value,
    ) -> FilingPlan {
        filing_plan(&FilingInput {
            graph,
            subgraphs,
            state,
            previous_state: Some(previous),
        })
    }

    fn conditioned_refund(key: &str, order: &str, amount: u64) -> Value {
        json!({ "subject": "tool:refund", "reason": format!("amount {amount} > 500"),
                "approvalKey": key, "input": { "amount": amount, "order": order },
                "condition": format!("amount {amount} > 500") })
    }

    #[test]
    fn a_second_conditioned_call_of_the_same_tool_after_a_resume_is_filed() {
        // ADR 0046: the agent asked `refund(A)`, A was filed, approved and ran; then it asked
        // `refund(B)`. Both waits read `tool:refund` at `assistant` — B must still be filed, and
        // the ids stashed for A dropped, or nobody ever sees B.
        let key_a = format!("refund#{}", "a".repeat(64));
        let key_b = format!("refund#{}", "b".repeat(64));
        let previous = suspended(
            "assistant",
            json!({ "agentResult": { "approvalRequests": [conditioned_refund(&key_a, "A", 600)] },
                    "__approvalIds": ["id-a"] }),
        );
        let after = suspended(
            "assistant",
            json!({ "agentResult": { "approvalRequests": [conditioned_refund(&key_b, "B", 700)] },
                    "__approvalIds": ["id-a"] }),
        );
        let plan = resumed(gated_agent(), None, previous, after);
        assert!(plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| request.subject.approval_key.as_deref())
            .collect();
        assert_eq!(filed, vec![Some(key_b.as_str())]);

        // The host stashed B's id: a resume — a redelivered job, a « resume » click — with A's
        // grant is refused while B is pending, so A does not run a second time.
        let waiting_on_b = suspended(
            "assistant",
            json!({ "agentResult": { "approvalRequests": [conditioned_refund(&key_b, "B", 700)] },
                    "__approvalIds": ["id-b"] }),
        );
        let approvals = json!({
            "id-a": { "status": "approved", "requestedBy": "assistant", "resolvedBy": "alice",
                      "subject": { "description": "tool:refund", "approvalKey": key_a } },
            "id-b": { "status": "pending", "requestedBy": "assistant",
                      "subject": { "description": "tool:refund", "approvalKey": key_b } }
        });
        let grant_a = json!([{ "name": "refund", "requestedBy": "assistant", "resolvedBy": "alice",
                               "key": key_a }]);
        assert_eq!(approvals_to_check(&waiting_on_b), vec!["id-b".to_owned()]);
        let problems = check(waiting_on_b, approvals, grant_a);
        assert!(
            problems.contains(&"request id-b (tool:refund) is still pending".to_owned()),
            "{problems:?}"
        );
    }

    #[test]
    fn the_same_call_asked_again_keeps_its_stash() {
        // Unchanged: a wait that holds the very same call (a rejected tool asked for again) is
        // the same wait — nothing is filed twice. Only the call each request holds is new.
        let key = format!("refund#{}", "a".repeat(64));
        let wait = json!({ "agentResult": { "approvalRequests": [conditioned_refund(&key, "A", 600)] },
                           "__approvalIds": ["id-a"] });
        let plan = resumed(
            gated_agent(),
            None,
            suspended("assistant", wait.clone()),
            suspended("assistant", wait),
        );
        assert_eq!(plan, FilingPlan::default());
    }

    #[test]
    fn a_wait_on_a_call_its_resume_granted_is_filed_again() {
        // ADR 0051 review R1: the agent asked `refund(A)`, A was filed and approved, and the resume
        // gave A's key back (`__approvedTools`). The resumed run waits on `refund(A)` again: that
        // grant was spent by A's execution (a key grant is spent by its call, ADR 0051 D5), so this
        // is a new request — a person decides it again, and the stash of the first is dropped.
        // The key the engine files for A (ADR 0051 D1 recomputes it from the input).
        let key_a = ailu_agents_core::tools::approval_key(
            "refund",
            true,
            &json!({ "amount": 600, "order": "A" }),
        );
        let wait = |granted: Value| {
            suspended(
                "assistant",
                json!({ "agentResult": { "approvalRequests": [conditioned_refund(&key_a, "A", 600)] },
                        "__approvalIds": ["id-a"], "__approvedTools": granted }),
            )
        };
        let plan = resumed(
            gated_agent(),
            None,
            wait(json!([])),
            wait(json!([key_a.clone()])),
        );
        assert!(plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| request.subject.approval_key.as_deref())
            .collect();
        assert_eq!(filed, vec![Some(key_a.as_str())]);

        // Re-driving the kept state (no resume) files nothing again: its stash is current.
        let kept = filing_plan(&FilingInput {
            graph: gated_agent(),
            subgraphs: None,
            state: wait(json!([key_a.clone()])),
            previous_state: None,
        });
        assert_eq!(kept, FilingPlan::default());
        // A grant of another call does not make it new: the same wait keeps its stash.
        let other = ailu_agents_core::tools::approval_key("refund", true, &json!({ "order": "B" }));
        let plan = resumed(gated_agent(), None, wait(json!([])), wait(json!([other])));
        assert_eq!(plan, FilingPlan::default());
    }

    #[test]
    fn a_child_that_moves_on_to_its_next_gate_is_filed() {
        // The parent waits at its subgraph node both times; its child moved from `c_first` to
        // `c_second`.
        let parent = graph(json!([
            { "id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child" }
        ]));
        let child = json!({ "id": "child", "version": "1", "name": "child", "channels": {},
            "nodes": [{ "id": "c_first", "type": "human-gate", "label": "c_first" },
                      { "id": "c_second", "type": "human-gate", "label": "c_second" }],
            "edges": [], "entryNodeId": "c_first" });
        let at = |gate: &str| {
            suspended(
                "sub",
                json!({ "__approvalIds": ["id-1"], "__subgraphStates": { "run-1:sub": {
                    "runId": "run-1:sub", "graphId": "child", "currentNodeId": gate,
                    "status": "suspended", "channels": {}, "version": 1,
                    "createdAt": "0", "updatedAt": "0" } } }),
            )
        };
        let plan = resumed(parent, Some(vec![child]), at("c_first"), at("c_second"));
        assert!(plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| {
                (
                    request.run_id.as_str(),
                    request.subject.description.as_str(),
                )
            })
            .collect();
        assert_eq!(filed, vec![("run-1:sub", "gate:run-1:sub:c_second")]);
    }

    #[test]
    fn a_gate_a_loop_comes_back_to_is_filed_again() {
        // A resume passes a human gate — it always advances — so a resume from a state waiting at
        // `review` that waits at `review` again came back to it through a loop: a new visit, which
        // a person decides again. The earlier approval is not carried over.
        let wait = json!({ "__approvalIds": ["gate-1"] });
        let plan = resumed(
            gated_agent(),
            None,
            suspended("review", wait.clone()),
            suspended("review", wait),
        );
        assert!(plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| request.subject.description.as_str())
            .collect();
        assert_eq!(filed, vec!["gate:review"]);
    }

    #[test]
    fn a_child_gate_a_loop_comes_back_to_is_filed_again() {
        let parent = graph(json!([
            { "id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child" }
        ]));
        let child = json!({ "id": "child", "version": "1", "name": "child", "channels": {},
            "nodes": [{ "id": "c_review", "type": "human-gate", "label": "c_review" }],
            "edges": [], "entryNodeId": "c_review" });
        let waiting = suspended(
            "sub",
            json!({ "__approvalIds": ["id-1"], "__subgraphStates": { "run-1:sub": {
                "runId": "run-1:sub", "graphId": "child", "currentNodeId": "c_review",
                "status": "suspended", "channels": {}, "version": 1,
                "createdAt": "0", "updatedAt": "0" } } }),
        );
        let plan = resumed(parent, Some(vec![child]), waiting.clone(), waiting);
        assert!(plan.clear_approval_ids);
        let filed: Vec<_> = plan
            .requests
            .iter()
            .map(|request| request.subject.description.as_str())
            .collect();
        assert_eq!(filed, vec!["gate:run-1:sub:c_review"]);
    }

    #[test]
    fn a_resume_from_an_agent_wait_back_at_the_same_agent_wait_is_not_a_gate_visit() {
        // The rule is for human gates only: an agent that asks for the same call again after
        // its request was rejected keeps its stash (child-workflow-approval.rust.test.ts).
        let wait = json!({ "agentResult": { "approvalRequests": [{ "subject": "tool:refund" }] },
                           "__approvalIds": ["id-1"] });
        let plan = resumed(
            gated_agent(),
            None,
            suspended("assistant", wait.clone()),
            suspended("assistant", wait),
        );
        assert_eq!(plan, FilingPlan::default());
    }

    #[test]
    fn a_tool_request_of_a_child_run_is_refused_not_filed() {
        // R6: no grant can reach a child run yet, so a person approving its gated call would
        // loop the run. Nothing is filed, and a resume is refused with the plan's reason first,
        // whatever was stashed before.
        let parent = graph(json!([
            { "id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child" }
        ]));
        let child = json!({ "id": "child", "version": "1", "name": "child", "channels": {},
            "nodes": [{ "id": "c_agent", "type": "agent", "label": "c_agent",
                        "metadata": { "agent": { "toolNames": ["refund"],
                                                 "suspendForApproval": true } } }],
            "edges": [], "entryNodeId": "c_agent" });
        let waiting = |stash: Value| {
            suspended(
                "sub",
                json!({ "__approvalIds": stash, "__subgraphStates": { "run-1:sub": {
                    "runId": "run-1:sub", "graphId": "child", "currentNodeId": "c_agent",
                    "status": "suspended", "version": 1, "createdAt": "0", "updatedAt": "0",
                    "channels": { "agentResult": {
                        "approvalRequests": [{ "subject": "tool:refund" }] } } } } }),
            )
        };
        let plan = filing_plan(&FilingInput {
            graph: parent.clone(),
            subgraphs: Some(vec![child.clone()]),
            state: waiting(json!([])),
            previous_state: None,
        });
        assert!(plan.requests.is_empty());
        assert_eq!(plan.refusal.as_deref(), Some(CHILD_TOOL_GRANT_REFUSAL));

        let approvals = json!({ "id-1": { "status": "approved", "requestedBy": "run-1:sub:c_agent",
            "resolvedBy": "alice", "subject": { "description": "tool:refund" } } });
        let problems = resume_problems(
            &serde_json::from_value(json!({
                "graph": parent, "subgraphs": [child], "state": waiting(json!(["id-1"])),
                "approvals": approvals,
                "approvedTools": [{ "name": "refund", "requestedBy": "run-1:sub:c_agent",
                                    "resolvedBy": "alice" }]
            }))
            .expect("check input parses"),
        );
        assert_eq!(problems, vec![CHILD_TOOL_GRANT_REFUSAL.to_owned()]);
    }

    #[test]
    fn the_runs_own_tool_request_and_a_childs_gate_are_still_filed() {
        // R6 is about a child's TOOL only: the run's own gated call and a child's human gate
        // are decided as before, with no refusal in the plan.
        let plan = filing_plan(&FilingInput {
            graph: gated_agent(),
            subgraphs: None,
            state: suspended(
                "assistant",
                json!({ "agentResult": { "approvalRequests": [{ "subject": "tool:refund" }] } }),
            ),
            previous_state: None,
        });
        assert_eq!(plan.refusal, None);
        assert_eq!(plan.requests.len(), 1);
        assert_eq!(
            serde_json::to_value(&plan).unwrap().get("refusal"),
            None,
            "the plan's JSON is unchanged when nothing is refused"
        );
    }

    #[test]
    fn a_run_driven_again_without_a_resume_does_not_file_twice() {
        // No previous state: the host re-drives the state it kept. Its ids are those of the wait
        // it is in: nothing is filed again.
        let plan = filing_plan(&FilingInput {
            graph: gated_agent(),
            subgraphs: None,
            state: suspended(
                "assistant",
                json!({ "agentResult": { "approvalRequests": [{ "subject": "tool:refund" }] },
                        "__approvalIds": ["id-1"] }),
            ),
            previous_state: None,
        });
        assert_eq!(plan, FilingPlan::default());
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
