//! Catalog graphs to an [`EngineSpec`] (ADR 0045 D3.2).
//!
//! A catalog graph is a plain `GraphDefinition` (authored in the Studio, persisted as data) whose
//! nodes carry their wiring in `node.metadata`:
//!
//! - `component: { kind, params }` — a native component node;
//! - `agent: { provider?, model?, tier?, system?, toolNames?, … }` — an agent node;
//! - `mapAgents: { overChannel, joinAt, subAgent, suspendForApproval? }` — a dynamic fan-out node.
//!
//! [`catalog_spec`] reads those carriers in the engine, for every SDK: an SDK hands it the
//! definition and the names of its bindings, then drives [`crate::run`] with the spec it returns,
//! instead of assembling the spec itself. Human gates and subgraph nodes run natively; every other
//! node without a carrier is a host node — the host's step when it binds one, an empty update
//! otherwise. A subgraph's nodes are read the same way and keyed by their global node id, since a
//! child run shares the parent's registries.
//!
//! It works on JSON values, the way the TypeScript SDK did before it: a carrier field the engine
//! does not interpret is copied verbatim, so a field of the wrong type (a `provider` that is not a
//! string, …) still reaches the spec, and [`crate::run`] refuses the spec when it parses it —
//! exactly as before. The golden cases in `tests/fixtures/catalog_spec_golden.json` were produced
//! by the TypeScript implementation this module replaces.

use std::collections::BTreeSet;

use ailu_agents_core::DEFAULT_AGENT_OUTPUT_CHANNEL;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::spec::EngineSpec;

/// The provider of an agent carrier that names none.
pub const DEFAULT_CATALOG_PROVIDER: &str = "anthropic";

/// What the host gives [`catalog_spec`] besides the definition: its bindings (by id or name only —
/// the functions stay in the host) and the per-run settings it passes through.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInput {
    /// The catalog `GraphDefinition`.
    pub graph: Value,
    /// Child definitions for the graph's `subgraph` nodes.
    #[serde(default)]
    pub subgraphs: Option<Vec<Value>>,
    /// The node ids the host binds a step to. Each must name a plain node, once.
    #[serde(default)]
    pub host_nodes: Option<Vec<String>>,
    /// The tool names the host backs (`EngineSpec.jsToolNames`).
    #[serde(default)]
    pub host_tools: Option<Vec<String>>,
    /// Passed through as `EngineSpec.providerKeys`.
    #[serde(default)]
    pub provider_keys: Option<Value>,
    /// Passed through as `EngineSpec.fsPolicy`.
    #[serde(default)]
    pub fs_policy: Option<Value>,
    /// Passed through as `EngineSpec.skills`.
    #[serde(default)]
    pub skills: Option<Value>,
}

/// The spec of a catalog graph, and what the host should tell its user about it.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogSpec {
    /// The `EngineSpec` wire value (camelCase), without the per-call fields (`runId`,
    /// `initialData`, `state`, …) the host adds for each entry.
    pub spec: Value,
    /// Carriers the engine ignored, worded for a person (a malformed `mapAgents`, …). The run
    /// still works without them; the host prints them.
    pub warnings: Vec<String>,
}

/// Why a catalog graph has no spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    /// A host node binding names no plain node of the graph or its subgraphs, or one twice.
    HostNodeBinding { node_id: String, reason: String },
    /// A carrier the spec cannot be built from — a `toolSpecs` that is not a list, or that holds
    /// a `null`.
    InvalidCarrier { node_id: String, message: String },
    /// The definition is not a graph: no `nodes` list, a node that is not an object or has no
    /// string id.
    InvalidDefinition(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::HostNodeBinding { node_id, reason } => {
                write!(f, "host node '{node_id}' can't be bound: {reason}")
            }
            CatalogError::InvalidCarrier { node_id, message } => {
                write!(f, "node '{node_id}': {message}")
            }
            CatalogError::InvalidDefinition(message) => {
                write!(f, "invalid catalog graph: {message}")
            }
        }
    }
}

impl std::error::Error for CatalogError {}

impl CatalogError {
    /// The error as the bindings send it: `{ kind, message, nodeId?, reason? }`, `kind` being
    /// `"hostNodeBinding"`, `"invalidCarrier"` or `"invalidDefinition"` — each SDK raises its own
    /// error type for it.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        match self {
            CatalogError::HostNodeBinding { node_id, reason } => json!({
                "kind": "hostNodeBinding",
                "message": self.to_string(),
                "nodeId": node_id,
                "reason": reason,
            }),
            CatalogError::InvalidCarrier { node_id, .. } => json!({
                "kind": "invalidCarrier",
                "message": self.to_string(),
                "nodeId": node_id,
            }),
            CatalogError::InvalidDefinition(_) => json!({
                "kind": "invalidDefinition",
                "message": self.to_string(),
            }),
        }
    }
}

/// `typeof v === "object" && v !== null && !Array.isArray(v)` — a JSON object.
fn as_record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// The non-empty string at `key`, if any.
fn non_empty_str<'a>(record: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// The value at `key` unless it is absent or `null` — JavaScript's `record[key] ?? fallback`.
fn or_default(record: &Map<String, Value>, key: &str, fallback: Value) -> Value {
    match record.get(key) {
        Some(Value::Null) | None => fallback,
        Some(value) => value.clone(),
    }
}

/// A node's component carrier, `{ kind, params }`: `kind` a non-empty string, `params` an object
/// (anything else reads as `{}`).
#[must_use]
pub fn read_component_carrier(metadata: Option<&Value>) -> Option<Value> {
    let component = as_record(metadata.and_then(|metadata| metadata.get("component")))?;
    let kind = non_empty_str(component, "kind")?;
    let params = as_record(component.get("params"))
        .cloned()
        .unwrap_or_default();
    Some(json!({ "kind": kind, "params": params }))
}

/// A node's agent carrier: any object under `metadata.agent`.
#[must_use]
pub fn read_agent_carrier(metadata: Option<&Value>) -> Option<&Map<String, Value>> {
    as_record(metadata.and_then(|metadata| metadata.get("agent")))
}

/// A node's mapAgents carrier, read.
#[derive(Clone, Debug, PartialEq)]
pub struct MapAgentCarrier<'a> {
    pub over_channel: &'a str,
    pub join_at: &'a str,
    pub sub_agent: &'a Map<String, Value>,
    pub suspend_for_approval: bool,
}

/// A node's mapAgents carrier: `overChannel` and `joinAt` non-empty strings, `subAgent` an object;
/// `suspendForApproval` is true only when it is `true`.
#[must_use]
pub fn read_map_agent_carrier(metadata: Option<&Value>) -> Option<MapAgentCarrier<'_>> {
    let map = as_record(metadata.and_then(|metadata| metadata.get("mapAgents")))?;
    Some(MapAgentCarrier {
        over_channel: non_empty_str(map, "overChannel")?,
        join_at: non_empty_str(map, "joinAt")?,
        sub_agent: as_record(map.get("subAgent"))?,
        suspend_for_approval: map.get("suspendForApproval") == Some(&Value::Bool(true)),
    })
}

/// Fields an agent carrier passes to the spec unchanged, under the spec's name.
const PASSTHROUGH_AGENT_FIELDS: [(&str, &str); 17] = [
    ("model", "model"),
    ("tier", "tier"),
    ("baseURL", "baseUrl"),
    ("apiKeyEnv", "apiKeyEnv"),
    ("system", "system"),
    ("maxIterations", "maxIterations"),
    ("outputStyle", "outputStyle"),
    ("contextBudget", "contextBudget"),
    ("todosChannel", "todosChannel"),
    ("inputBlocksChannel", "inputBlocksChannel"),
    ("visibleChannels", "visibleChannels"),
    ("webSearch", "webSearch"),
    ("memory", "memory"),
    ("skills", "skills"),
    ("enableFs", "enableFs"),
    ("resolvedMiddleware", "resolvedMiddleware"),
    // ADR 0046: conditions on a gated tool's arguments — validated by the engine when it builds
    // the agent, like every other field.
    ("approvalWhen", "approvalWhen"),
];

/// An agent carrier as the spec's `AgentSpec`: provider `"anthropic"` when it names none, empty
/// tool and approval lists, the default output channel, `suspendForApproval` only when `true`, and
/// each `toolSpecs` entry's `jsonSchema` as `inputSchema`. A field the carrier leaves out stays
/// out; a field it sets to `null` stays `null` (the engine reads both as unset).
fn agent_spec(node_id: &str, carrier: &Map<String, Value>) -> Result<Value, CatalogError> {
    let mut spec = Map::new();
    spec.insert(
        "provider".to_owned(),
        or_default(carrier, "provider", json!(DEFAULT_CATALOG_PROVIDER)),
    );
    for (from, to) in PASSTHROUGH_AGENT_FIELDS {
        if let Some(value) = carrier.get(from) {
            spec.insert(to.to_owned(), value.clone());
        }
    }
    spec.insert(
        "toolNames".to_owned(),
        or_default(carrier, "toolNames", json!([])),
    );
    if let Some(tool_specs) = tool_specs(node_id, carrier.get("toolSpecs"))? {
        spec.insert("toolSpecs".to_owned(), tool_specs);
    }
    spec.insert(
        "suspendForApproval".to_owned(),
        Value::Bool(carrier.get("suspendForApproval") == Some(&Value::Bool(true))),
    );
    spec.insert(
        "approvalToolNames".to_owned(),
        or_default(carrier, "approvalToolNames", json!([])),
    );
    spec.insert(
        "outputChannel".to_owned(),
        or_default(
            carrier,
            "outputChannel",
            json!(DEFAULT_AGENT_OUTPUT_CHANNEL),
        ),
    );
    Ok(Value::Object(spec))
}

/// `toolSpecs` as the spec's `[{ name, description, inputSchema }]`. Absent or `null` → absent. A
/// value that is not a list, or a `null` entry, is refused (the TypeScript SDK threw on both); an
/// entry that is not an object becomes `{}`, which the engine refuses for its missing `name`.
fn tool_specs(node_id: &str, value: Option<&Value>) -> Result<Option<Value>, CatalogError> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(entries)) => entries,
        Some(_) => {
            return Err(CatalogError::InvalidCarrier {
                node_id: node_id.to_owned(),
                message: "the agent's toolSpecs must be a list".to_owned(),
            })
        }
    };
    let mut specs = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut spec = Map::new();
        match entry {
            Value::Null => {
                return Err(CatalogError::InvalidCarrier {
                    node_id: node_id.to_owned(),
                    message: "the agent's toolSpecs holds a null entry".to_owned(),
                })
            }
            Value::Object(tool) => {
                for (from, to) in [
                    ("name", "name"),
                    ("description", "description"),
                    ("jsonSchema", "inputSchema"),
                ] {
                    if let Some(value) = tool.get(from) {
                        spec.insert(to.to_owned(), value.clone());
                    }
                }
            }
            _ => {}
        }
        specs.push(Value::Object(spec));
    }
    Ok(Some(Value::Array(specs)))
}

/// Push `item` unless it is already there — a JavaScript `Set` in insertion order.
fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|existing| existing == item) {
        list.push(item.to_owned());
    }
}

/// Entries keyed by node id in first-insertion order, a later insertion replacing the value in
/// place — a JavaScript `Map`, so the spec is built in the order the TypeScript SDK built it.
struct ByNodeId<T>(Vec<(String, T)>);

impl<T> ByNodeId<T> {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn insert(&mut self, id: &str, value: T) {
        match self.0.iter_mut().find(|(existing, _)| existing == id) {
            Some(entry) => entry.1 = value,
            None => self.0.push((id.to_owned(), value)),
        }
    }
}

/// The nodes of a definition, each with its id.
fn graph_nodes<'a>(
    graph: &'a Value,
    what: &str,
) -> Result<Vec<(&'a str, &'a Value)>, CatalogError> {
    let nodes = graph
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| CatalogError::InvalidDefinition(format!("{what} has no `nodes` list")))?;
    nodes
        .iter()
        .map(|node| {
            node.get("id")
                .and_then(Value::as_str)
                .map(|id| (id, node))
                .ok_or_else(|| {
                    CatalogError::InvalidDefinition(format!(
                        "{what} has a node without a string `id`"
                    ))
                })
        })
        .collect()
}

/// Build the [`CatalogSpec`] of a catalog graph (see the module docs).
///
/// Nodes are read in order, the graph's then each subgraph's; a later node with an id seen
/// before overwrites that id's agent, component or mapAgents entry. Per node, the first carrier
/// that reads wins: component, then mapAgents (a malformed one is reported in
/// [`CatalogSpec::warnings`] and ignored), then agent. A human gate or a subgraph node without a
/// carrier runs natively; any other node is a host node. Host node bindings are checked before
/// the agent carriers are turned into specs.
///
/// # Errors
///
/// [`CatalogError::InvalidDefinition`] when a definition has no node list or a node no string id,
/// [`CatalogError::HostNodeBinding`] when `host_nodes` names no plain node or one twice,
/// [`CatalogError::InvalidCarrier`] for a `toolSpecs` the spec cannot carry.
pub fn catalog_spec(input: &CatalogInput) -> Result<CatalogSpec, CatalogError> {
    let subgraphs = input.subgraphs.clone().unwrap_or_default();
    let mut agents: ByNodeId<&Map<String, Value>> = ByNodeId::new();
    let mut components: ByNodeId<Value> = ByNodeId::new();
    let mut map_agents: ByNodeId<MapAgentCarrier> = ByNodeId::new();
    let mut host_node_ids: Vec<String> = Vec::new();
    let mut node_ids: BTreeSet<&str> = BTreeSet::new();
    let mut warnings = Vec::new();

    let mut all_nodes = graph_nodes(&input.graph, "the graph")?;
    for (index, subgraph) in subgraphs.iter().enumerate() {
        all_nodes.extend(graph_nodes(subgraph, &format!("subgraph #{index}"))?);
    }

    for (id, node) in all_nodes {
        node_ids.insert(id);
        let metadata = node.get("metadata");
        if let Some(component) = read_component_carrier(metadata) {
            components.insert(id, component);
            continue;
        }
        // A mapAgents carrier wins over an agent carrier: a fan-out node is not itself an agent.
        if let Some(map) = read_map_agent_carrier(metadata) {
            map_agents.insert(id, map);
            continue;
        }
        // A node that carries a mapAgents key that does not read would otherwise fall through
        // silently and never fan out: say so.
        if as_record(metadata.and_then(|metadata| metadata.get("mapAgents"))).is_some() {
            warnings.push(format!(
                "node \"{id}\" has a malformed mapAgents carrier (needs overChannel, joinAt, subAgent) — it will NOT fan out."
            ));
        }
        if let Some(carrier) = read_agent_carrier(metadata) {
            agents.insert(id, carrier);
            continue;
        }
        let node_type = node.get("type").and_then(Value::as_str);
        if matches!(node_type, Some("human-gate" | "subgraph")) {
            // The runtime runs both natively: a gate suspends, a subgraph recurses.
            continue;
        }
        push_unique(&mut host_node_ids, id);
    }

    let mut bound: BTreeSet<&str> = BTreeSet::new();
    for node_id in input.host_nodes.iter().flatten() {
        let reason = if bound.contains(node_id.as_str()) {
            Some("it is bound twice")
        } else if !host_node_ids.contains(node_id) {
            Some(if node_ids.contains(node_id.as_str()) {
                "it is not a plain step — agents, components, human gates and subgraphs run on the engine"
            } else {
                "the graph and its subgraphs have no node with this id"
            })
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(CatalogError::HostNodeBinding {
                node_id: node_id.clone(),
                reason: reason.to_owned(),
            });
        }
        bound.insert(node_id);
    }

    let mut agent_specs = Map::new();
    for (id, carrier) in &agents.0 {
        agent_specs.insert(id.clone(), agent_spec(id, carrier)?);
    }
    let mut map_agent_specs = Map::new();
    for (id, map) in &map_agents.0 {
        map_agent_specs.insert(
            id.clone(),
            json!({
                "overChannel": map.over_channel,
                "joinAt": map.join_at,
                "agent": agent_spec(id, map.sub_agent)?,
                "suspendForApproval": map.suspend_for_approval,
            }),
        );
    }
    let component_specs: Map<String, Value> = components.0.into_iter().collect();

    let mut host_tools: Vec<String> = Vec::new();
    for name in input.host_tools.iter().flatten() {
        push_unique(&mut host_tools, name);
    }

    let spec = json!({
        "graph": input.graph,
        "subgraphs": subgraphs,
        "agents": agent_specs,
        "componentNodes": component_specs,
        "mapAgents": map_agent_specs,
        "hostNodeIds": host_node_ids,
        "jsToolNames": host_tools,
        "providerKeys": passthrough(input.provider_keys.as_ref(), json!({})),
        "fsPolicy": passthrough(input.fs_policy.as_ref(), json!([])),
        "skills": passthrough(input.skills.as_ref(), json!([])),
    });
    Ok(CatalogSpec { spec, warnings })
}

/// A per-run setting as given, or its empty value when absent or `null`.
fn passthrough(value: Option<&Value>, empty: Value) -> Value {
    match value {
        None | Some(Value::Null) => empty,
        Some(value) => value.clone(),
    }
}

/// [`catalog_spec`] as a typed [`EngineSpec`], for a Rust host. Refused when the spec does not
/// parse — the same refusal [`crate::run`] would give.
///
/// # Errors
///
/// The [`CatalogError`] message, or the spec's parse error.
pub fn spec_from_catalog(input: &CatalogInput) -> Result<(EngineSpec, Vec<String>), String> {
    let CatalogSpec { spec, warnings } = catalog_spec(input).map_err(|error| error.to_string())?;
    let spec = serde_json::from_value(spec)
        .map_err(|error| format!("invalid engine spec JSON: {error}"))?;
    Ok((spec, warnings))
}

/// [`catalog_spec`] for the language bindings: a [`CatalogInput`] as JSON in;
/// `{ "spec", "warnings" }` or `{ "error": { kind, message, nodeId?, reason? } }` as JSON out.
///
/// # Errors
///
/// Only when the input is not a JSON [`CatalogInput`].
pub fn catalog_spec_json(input_json: &str) -> Result<String, String> {
    let input: CatalogInput = serde_json::from_str(input_json)
        .map_err(|error| format!("invalid catalog input JSON: {error}"))?;
    let wire = match catalog_spec(&input) {
        Ok(CatalogSpec { spec, warnings }) => json!({ "spec": spec, "warnings": warnings }),
        Err(error) => json!({ "error": error.to_wire() }),
    };
    serde_json::to_string(&wire).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(nodes: Value) -> Value {
        json!({
            "id": "g",
            "version": "1",
            "name": "g",
            "channels": {},
            "nodes": nodes,
            "edges": [],
            "entryNodeId": "a"
        })
    }

    fn input(nodes: Value) -> CatalogInput {
        CatalogInput {
            graph: graph(nodes),
            ..CatalogInput::default()
        }
    }

    fn spec_of(nodes: Value) -> Value {
        catalog_spec(&input(nodes)).expect("spec builds").spec
    }

    #[test]
    fn an_agent_carrier_gets_the_defaults() {
        let spec = spec_of(json!([
            { "id": "a", "type": "agent", "label": "a", "metadata": { "agent": {} } }
        ]));
        assert_eq!(
            spec["agents"]["a"],
            json!({
                "provider": "anthropic",
                "toolNames": [],
                "suspendForApproval": false,
                "approvalToolNames": [],
                "outputChannel": "agentResult"
            })
        );
        assert_eq!(spec["hostNodeIds"], json!([]));
    }

    #[test]
    fn an_agent_carrier_passes_its_approval_conditions_through() {
        // ADR 0046: `approvalWhen` reaches the engine as written; the engine validates it when it
        // builds the agent, and reads `null` as no conditions.
        let spec = spec_of(json!([{
            "id": "a", "type": "agent", "label": "a",
            "metadata": { "agent": {
                "toolNames": ["refund"], "approvalToolNames": ["refund"],
                "approvalWhen": { "refund": [{ "argument": "amount", "above": 500 }] }
            } }
        }]));
        assert_eq!(
            spec["agents"]["a"]["approvalWhen"],
            json!({ "refund": [{ "argument": "amount", "above": 500 }] })
        );
        let parsed: crate::spec::AgentSpec =
            serde_json::from_value(spec["agents"]["a"].clone()).expect("the engine reads it");
        assert_eq!(parsed.approval_when["refund"][0].argument, "amount");

        let unset = spec_of(json!([{
            "id": "a", "type": "agent", "label": "a",
            "metadata": { "agent": { "approvalWhen": null } }
        }]));
        let parsed: crate::spec::AgentSpec =
            serde_json::from_value(unset["agents"]["a"].clone()).expect("null reads as unset");
        assert!(parsed.approval_when.is_empty());
    }

    #[test]
    fn null_reads_as_unset_where_the_sdk_defaulted_and_stays_null_elsewhere() {
        let spec = spec_of(json!([{
            "id": "a", "type": "agent", "label": "a",
            "metadata": { "agent": {
                "provider": null, "toolNames": null, "approvalToolNames": null,
                "outputChannel": null, "model": null, "suspendForApproval": "yes"
            } }
        }]));
        let agent = &spec["agents"]["a"];
        assert_eq!(agent["provider"], json!("anthropic"));
        assert_eq!(agent["toolNames"], json!([]));
        assert_eq!(agent["approvalToolNames"], json!([]));
        assert_eq!(agent["outputChannel"], json!("agentResult"));
        assert_eq!(agent["model"], Value::Null);
        assert_eq!(agent["suspendForApproval"], json!(false));
    }

    #[test]
    fn tool_specs_rename_json_schema_and_base_url_is_spelled_the_engine_way() {
        let spec = spec_of(json!([{
            "id": "a", "type": "agent", "label": "a",
            "metadata": { "agent": {
                "baseURL": "http://localhost:8000/v1",
                "toolSpecs": [
                    { "name": "refund", "description": "Refunds.", "jsonSchema": { "type": "object" }, "extra": 1 },
                    "not-an-object"
                ]
            } }
        }]));
        let agent = &spec["agents"]["a"];
        assert_eq!(agent["baseUrl"], json!("http://localhost:8000/v1"));
        assert!(agent.get("baseURL").is_none());
        assert_eq!(
            agent["toolSpecs"],
            json!([
                { "name": "refund", "description": "Refunds.", "inputSchema": { "type": "object" } },
                {}
            ])
        );
    }

    #[test]
    fn tool_specs_that_are_not_a_list_or_hold_null_are_refused() {
        for tool_specs in [
            json!({ "name": "x" }),
            json!("x"),
            json!(false),
            json!([null]),
        ] {
            let error = catalog_spec(&input(json!([{
                "id": "a", "type": "agent", "label": "a",
                "metadata": { "agent": { "toolSpecs": tool_specs } }
            }])))
            .expect_err("refused");
            assert!(
                matches!(error, CatalogError::InvalidCarrier { ref node_id, .. } if node_id == "a"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_component_wins_then_map_agents_then_agent() {
        let spec = spec_of(json!([
            { "id": "c", "type": "action", "label": "c", "metadata": {
                "component": { "kind": "promptBuilder", "params": [1] },
                "agent": {}
            } },
            { "id": "m", "type": "action", "label": "m", "metadata": {
                "mapAgents": { "overChannel": "items", "joinAt": "out", "subAgent": { "model": "x" } },
                "agent": {}
            } },
            { "id": "a", "type": "action", "label": "a", "metadata": {
                "component": { "kind": "" },
                "agent": { "provider": "mistral" }
            } }
        ]));
        assert_eq!(
            spec["componentNodes"]["c"],
            json!({ "kind": "promptBuilder", "params": {} })
        );
        assert_eq!(spec["mapAgents"]["m"]["overChannel"], json!("items"));
        assert_eq!(spec["mapAgents"]["m"]["agent"]["model"], json!("x"));
        assert_eq!(spec["mapAgents"]["m"]["suspendForApproval"], json!(false));
        assert_eq!(spec["agents"]["a"]["provider"], json!("mistral"));
        assert!(spec["agents"].get("c").is_none());
        assert!(spec["agents"].get("m").is_none());
    }

    #[test]
    fn a_malformed_map_agents_carrier_is_reported_and_the_node_falls_through() {
        let built = catalog_spec(&input(json!([
            { "id": "m", "type": "action", "label": "m", "metadata": {
                "mapAgents": { "overChannel": "", "joinAt": "out", "subAgent": {} }
            } },
            { "id": "n", "type": "action", "label": "n", "metadata": {
                "mapAgents": { "overChannel": "items", "joinAt": "out" },
                "agent": {}
            } },
            { "id": "s", "type": "action", "label": "s", "metadata": { "mapAgents": "items" } }
        ])))
        .expect("spec builds");
        assert_eq!(
            built.warnings,
            vec![
                "node \"m\" has a malformed mapAgents carrier (needs overChannel, joinAt, subAgent) — it will NOT fan out.".to_owned(),
                "node \"n\" has a malformed mapAgents carrier (needs overChannel, joinAt, subAgent) — it will NOT fan out.".to_owned(),
            ]
        );
        assert_eq!(built.spec["hostNodeIds"], json!(["m", "s"]));
        assert!(built.spec["agents"].get("n").is_some());
    }

    #[test]
    fn gates_and_subgraph_nodes_run_natively_and_subgraph_nodes_are_flattened() {
        let built = catalog_spec(&CatalogInput {
            graph: graph(json!([
                { "id": "a", "type": "action", "label": "a" },
                { "id": "gate", "type": "human-gate", "label": "gate" },
                { "id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child" }
            ])),
            subgraphs: Some(vec![json!({
                "id": "child", "version": "1", "name": "child", "channels": {},
                "nodes": [
                    { "id": "c1", "type": "tool", "label": "c1" },
                    { "id": "c2", "type": "agent", "label": "c2", "metadata": { "agent": {} } },
                    { "id": "a", "type": "action", "label": "a" }
                ],
                "edges": [], "entryNodeId": "c1"
            })]),
            ..CatalogInput::default()
        })
        .expect("spec builds");
        assert_eq!(built.spec["hostNodeIds"], json!(["a", "c1"]));
        assert!(built.spec["agents"].get("c2").is_some());
        assert_eq!(built.spec["subgraphs"][0]["id"], json!("child"));
    }

    #[test]
    fn host_node_bindings_must_name_a_plain_node_once() {
        let nodes = json!([
            { "id": "a", "type": "action", "label": "a" },
            { "id": "agent", "type": "agent", "label": "agent", "metadata": { "agent": {} } }
        ]);
        let bind = |ids: &[&str]| {
            catalog_spec(&CatalogInput {
                graph: graph(nodes.clone()),
                host_nodes: Some(ids.iter().map(|id| (*id).to_owned()).collect()),
                ..CatalogInput::default()
            })
        };
        assert!(bind(&["a"]).is_ok());
        let reason = |ids: &[&str]| match bind(ids) {
            Err(CatalogError::HostNodeBinding { node_id, reason }) => (node_id, reason),
            other => panic!("expected a binding error, got {other:?}"),
        };
        assert_eq!(
            reason(&["a", "a"]),
            ("a".to_owned(), "it is bound twice".to_owned())
        );
        assert_eq!(reason(&["agent"]).1, "it is not a plain step — agents, components, human gates and subgraphs run on the engine");
        assert_eq!(
            reason(&["nope"]).1,
            "the graph and its subgraphs have no node with this id"
        );
    }

    #[test]
    fn per_run_settings_pass_through_and_tools_are_deduplicated() {
        let built = catalog_spec(&CatalogInput {
            graph: graph(json!([{ "id": "a", "type": "action", "label": "a" }])),
            host_tools: Some(vec!["b".to_owned(), "a".to_owned(), "b".to_owned()]),
            provider_keys: Some(json!({ "openai": "sk" })),
            fs_policy: Some(Value::Null),
            ..CatalogInput::default()
        })
        .expect("spec builds");
        assert_eq!(built.spec["jsToolNames"], json!(["b", "a"]));
        assert_eq!(built.spec["providerKeys"], json!({ "openai": "sk" }));
        assert_eq!(built.spec["fsPolicy"], json!([]));
        assert_eq!(built.spec["skills"], json!([]));
        assert_eq!(built.spec["subgraphs"], json!([]));
    }

    #[test]
    fn a_definition_without_nodes_or_string_ids_is_refused() {
        let no_nodes = CatalogInput {
            graph: json!({ "id": "g" }),
            ..CatalogInput::default()
        };
        assert!(matches!(
            catalog_spec(&no_nodes),
            Err(CatalogError::InvalidDefinition(_))
        ));
        assert!(matches!(
            catalog_spec(&input(json!([{ "id": 5, "type": "action", "label": "x" }]))),
            Err(CatalogError::InvalidDefinition(_))
        ));
    }

    #[test]
    fn the_typed_spec_parses_and_a_carrier_of_the_wrong_type_is_refused_there() {
        let (spec, warnings) = spec_from_catalog(&input(json!([
            { "id": "a", "type": "agent", "label": "a", "metadata": { "agent": { "model": "m" } } },
            { "id": "b", "type": "action", "label": "b" }
        ])))
        .expect("spec parses");
        assert!(warnings.is_empty());
        assert_eq!(spec.agents["a"].model.as_deref(), Some("m"));
        assert_eq!(spec.host_node_ids, vec!["b".to_owned()]);

        let error = spec_from_catalog(&input(json!([
            { "id": "a", "type": "agent", "label": "a", "metadata": { "agent": { "provider": 42 } } }
        ])))
        .expect_err("refused");
        assert!(error.starts_with("invalid engine spec JSON"), "{error}");
    }

    #[test]
    fn the_json_entry_returns_the_spec_or_the_error_and_refuses_bad_input() {
        let ok: Value = serde_json::from_str(
            &catalog_spec_json(&json!({ "graph": graph(json!([])) }).to_string()).unwrap(),
        )
        .unwrap();
        assert_eq!(ok["warnings"], json!([]));
        assert_eq!(ok["spec"]["hostNodeIds"], json!([]));

        let refused: Value = serde_json::from_str(
            &catalog_spec_json(
                &json!({ "graph": graph(json!([])), "hostNodes": ["x"] }).to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(refused["error"]["kind"], json!("hostNodeBinding"));
        assert_eq!(refused["error"]["nodeId"], json!("x"));

        assert!(catalog_spec_json("{").is_err());
    }
}
