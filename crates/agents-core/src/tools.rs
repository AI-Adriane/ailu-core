//! Tool definitions, handlers, and the in-memory registry — the Rust port of
//! `@ailu-ai/agents-core`'s `tools.ts`. Schemas are plain JSON Schema values
//! (no Zod here); validation belongs to the caller and the LLM contract.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Describes a tool an agent may call. Serializes camelCase (`requiresApproval`,
/// `inputSchema`), wire-compatible with the TS `ToolDefinition` subset.
///
/// Only tools that carry an `input_schema` are advertised to the LLM as native
/// tool definitions; schema-less tools remain callable through the `ACTION:`
/// text protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// When `true`, an agent never executes this tool on its own: it records an
    /// approval request instead, unless the name was explicitly granted.
    pub requires_approval: bool,
    /// JSON Schema advertised to the LLM so it can emit native tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// When `true` (with `requires_approval`), the approval is **content-scoped**
    /// (ADR 0024 phase 2c): the grant is pinned to the exact call via
    /// `"<name>#<sha256(input)>"` (see [`approval_key`]), so approving one call does NOT
    /// unlock a different input. Used by the guarded fs writes. Default `false`
    /// (name-only grant).
    #[serde(default)]
    pub content_scoped: bool,
    /// Conditions on the call's arguments (ADR 0046, 0048). Empty — the default — keeps the gate
    /// decided by the name alone. Set (on a `requires_approval` tool), the gate opens per call:
    /// when an argument is absent, not of its test's type, above its threshold or one of its named
    /// values — and then the grant is that call (`"<name>#<sha256(input)>"`, like a
    /// content-scoped tool). See [`crossings`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_conditions: Vec<ApprovalCondition>,
}

/// What one human approval of an agent's gated tool unlocks (ADR 0051 D4).
///
/// - [`ApprovalScope::Tool`] — the default, today's behaviour: a gate decided by the tool's name is
///   granted by the name (a content-scoped or conditioned call is still granted per call).
/// - [`ApprovalScope::Call`] — every approval-gated tool of the agent is granted per call: the grant
///   is the call key `"<name>#<sha256(canonical input)>"`, a grant by name unlocks nothing, a call
///   with other arguments opens a new gate, and a grant is spent by the one execution of its call
///   (ADR 0051 D5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    #[default]
    Tool,
    Call,
}

/// One condition on a gated tool's arguments: the call needs approval when the top-level input
/// field `argument` crosses the condition's one test — a number above `above` (ADR 0046), or a
/// string equal to one of `in` (ADR 0048) — or is absent, or not of the test's type. Data, never
/// an expression. Exactly one test: a condition with both or neither is refused when the agent is
/// built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCondition {
    pub argument: String,
    /// ADR 0046: crossed by a number above this threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    /// ADR 0048: crossed by a string equal — byte for byte — to one of these values. `in` on the
    /// wire.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<String>>,
}

impl ApprovalCondition {
    /// `{ argument, above }` — crossed above a threshold (ADR 0046).
    pub fn above(argument: impl Into<String>, threshold: f64) -> Self {
        Self {
            argument: argument.into(),
            above: Some(threshold),
            one_of: None,
        }
    }

    /// `{ argument, in }` — crossed by one of the named values (ADR 0048).
    pub fn one_of<V: Into<String>>(
        argument: impl Into<String>,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        Self {
            argument: argument.into(),
            above: None,
            one_of: Some(values.into_iter().map(Into::into).collect()),
        }
    }
}

/// What of a call crosses its tool's conditions, one entry per condition crossed — `"amount 600 >
/// 500"`, `"agentName = \"Nordlys\""`, `"amount missing"`, `"amount not a number"` (`"600"` written
/// as text is not a number), `"agentName not a string"`. Empty when no condition is crossed: the
/// call then runs without a gate. Fail-closed: an argument that is absent or not of its test's type
/// crosses, and so does a condition that does not carry exactly one test.
pub fn crossings(conditions: &[ApprovalCondition], input: &Value) -> Vec<String> {
    conditions
        .iter()
        .filter_map(|condition| crossing(condition, input))
        .collect()
}

fn crossing(condition: &ApprovalCondition, input: &Value) -> Option<String> {
    let argument = condition.argument.as_str();
    let value = match input.get(argument) {
        None | Some(Value::Null) => return Some(format!("{argument} missing")),
        Some(value) => value,
    };
    match (condition.above, condition.one_of.as_deref()) {
        (Some(threshold), None) => match value.as_f64() {
            Some(number) if number > threshold => Some(format!("{argument} {value} > {threshold}")),
            Some(_) => None,
            None => Some(format!("{argument} not a number")),
        },
        (None, Some(values)) => match value.as_str() {
            Some(text) if values.iter().any(|named| named == text) => {
                Some(format!("{argument} = {value}"))
            }
            Some(_) => None,
            None => Some(format!("{argument} not a string")),
        },
        // Refused when the agent is built; should one reach a call anyway, it gates.
        _ => Some(format!("{argument} has no single test")),
    }
}

/// The approval grant key for a tool call. For an ordinary tool this is just the tool
/// name; for a **content-scoped** tool it is `"<name>#<sha256(canonical input JSON)>"`,
/// pinning the approval to the exact call (ADR 0024 phase 2c — a different path/content
/// hashes differently and re-gates, preventing over-grant).
///
/// The input is **canonicalized** (object keys sorted recursively) before hashing, so
/// the key is byte-stable across the suspend→resume round-trip **by construction** —
/// independent of `serde_json`'s `preserve_order` feature (defense-in-depth: a future
/// transitive dep enabling it must not be able to shift hashes).
pub fn approval_key(name: &str, content_scoped: bool, input: &Value) -> String {
    if !content_scoped {
        return name.to_owned();
    }
    call_key_of(name, input)
}

/// The canonical text of a call's arguments, the exact bytes a call key hashes (ADR 0051 D1):
/// every object's keys sorted by their UTF-8 bytes at every depth, arrays in order, compact, as
/// `serde_json` writes strings and numbers (a 64-bit integer exactly; any other number as the
/// shortest decimal that reads back to the same double, `40.0`, `1e21`). Frozen: it is what every
/// content-scoped (ADR 0024) and conditioned (ADR 0046) grant has hashed since it shipped.
pub fn call_input_of(input: &Value) -> String {
    canonical_json(input).to_string()
}

/// A call's identity, `"<name>#" + hex(sha256(call_input_of(input)))` (ADR 0051 D1): the grant
/// key of a call whose grant is the call, and what a host shows, signs and compares.
pub fn call_key_of(name: &str, input: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(call_input_of(input).as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{name}#{hex}")
}

/// [`call_input_of`] over the arguments' JSON text, for the bindings: a host passes the text, not
/// a parsed value, since a JavaScript or Python value has already lost what tells `40` from
/// `40.0`. The canonical text itself reads back to itself.
///
/// # Errors
///
/// When `input_json` is not JSON.
pub fn call_input_of_json(input_json: &str) -> Result<String, String> {
    let input: Value = serde_json::from_str(input_json)
        .map_err(|error| format!("invalid call input JSON: {error}"))?;
    Ok(call_input_of(&input))
}

/// [`call_key_of`] over the arguments' JSON text, for the bindings (see [`call_input_of_json`]).
///
/// # Errors
///
/// When `input_json` is not JSON.
pub fn call_key_of_json(name: &str, input_json: &str) -> Result<String, String> {
    let input: Value = serde_json::from_str(input_json)
        .map_err(|error| format!("invalid call input JSON: {error}"))?;
    Ok(call_key_of(name, &input))
}

/// Rebuild a JSON value with every object's keys in sorted order (recursively), so its
/// serialization is canonical regardless of how the original was constructed or which
/// `serde_json` map backing is in use.
fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_json(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

/// The boxed future a tool handler produces.
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// An async tool handler: JSON input in, JSON output (or an error message) out.
pub type ToolHandler = Box<dyn Fn(Value) -> ToolFuture + Send + Sync>;

/// Wrap a synchronous closure as an async [`ToolHandler`] — convenient for pure
/// tools and tests that don't await anything.
pub fn sync_tool<F>(f: F) -> ToolHandler
where
    F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
{
    Box::new(move |input| Box::pin(std::future::ready(f(input))))
}

/// A plain in-memory tool registry keyed by tool name. Registration takes
/// `&mut self`; lookups are `&self`, so a populated registry stays `Sync` and
/// can be shared behind an `Arc` by concurrent node handlers.
#[derive(Default)]
pub struct InMemoryToolRegistry {
    entries: HashMap<String, (ToolDefinition, ToolHandler)>,
}

impl InMemoryToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a tool under its definition's name.
    pub fn register(&mut self, definition: ToolDefinition, handler: ToolHandler) {
        self.entries
            .insert(definition.name.clone(), (definition, handler));
    }

    /// Look a tool up by name.
    pub fn resolve(&self, name: &str) -> Option<(&ToolDefinition, &ToolHandler)> {
        self.entries.get(name).map(|entry| (&entry.0, &entry.1))
    }

    /// Every registered tool definition (unordered).
    pub fn list(&self) -> Vec<&ToolDefinition> {
        self.entries.values().map(|entry| &entry.0).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn approval_key_is_order_independent_and_pins_content() {
        // Reordered keys → SAME canonical key (the over-grant guard must survive a
        // suspend→resume that re-serializes the input in a different key order).
        let a = approval_key("g", true, &json!({ "path": "x", "content": "Y" }));
        let reordered = approval_key("g", true, &json!({ "content": "Y", "path": "x" }));
        assert_eq!(a, reordered);
        // Different content → different key (a grant for one does not unlock the other).
        let other = approval_key("g", true, &json!({ "path": "x", "content": "Z" }));
        assert_ne!(a, other);
        // Shape: "<name>#<64 hex>".
        assert!(a.starts_with("g#"));
        assert_eq!(a.len(), "g#".len() + 64);
        // Non-content-scoped → bare name.
        assert_eq!(approval_key("g", false, &json!({ "path": "x" })), "g");
    }

    #[test]
    fn a_call_crosses_its_conditions_when_above_absent_or_not_a_number() {
        let conditions = vec![ApprovalCondition::above("amount", 500.0)];
        // At or below the threshold: no crossing, the call runs ungated.
        assert!(crossings(&conditions, &json!({ "amount": 500 })).is_empty());
        assert!(crossings(&conditions, &json!({ "amount": 120.5, "to": "x" })).is_empty());
        // Above: named with the value and the threshold.
        assert_eq!(
            crossings(&conditions, &json!({ "amount": 600 })),
            vec!["amount 600 > 500".to_owned()]
        );
        // Fail-closed: absent, null, written as text, or another type.
        assert_eq!(
            crossings(&conditions, &json!({})),
            vec!["amount missing".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "amount": null })),
            vec!["amount missing".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "amount": "600" })),
            vec!["amount not a number".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "amount": [600] })),
            vec!["amount not a number".to_owned()]
        );
        // Not an object at all: every argument is missing.
        assert_eq!(
            crossings(&conditions, &json!("600")),
            vec!["amount missing".to_owned()]
        );
        // Several conditions: every one crossed is named, in order.
        let two = vec![
            conditions[0].clone(),
            ApprovalCondition::above("quantity", 10.0),
        ];
        assert_eq!(
            crossings(&two, &json!({ "amount": 900, "quantity": 12 })),
            vec!["amount 900 > 500".to_owned(), "quantity 12 > 10".to_owned()]
        );
        assert!(crossings(&[], &json!({ "amount": 1e9 })).is_empty());
    }

    #[test]
    fn a_call_crosses_named_values_when_one_of_them_absent_or_not_a_string() {
        // ADR 0048: the gate opens for the named values only — byte for byte.
        let conditions = vec![ApprovalCondition::one_of(
            "agentName",
            ["Nordlys", "Veritas"],
        )];
        assert_eq!(
            crossings(
                &conditions,
                &json!({ "agentName": "Nordlys", "message": "quote" })
            ),
            vec!["agentName = \"Nordlys\"".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "agentName": "Veritas" })),
            vec!["agentName = \"Veritas\"".to_owned()]
        );
        // Another value — another case, spaces around it, another agent: no crossing.
        assert!(crossings(&conditions, &json!({ "agentName": "Other" })).is_empty());
        assert!(crossings(&conditions, &json!({ "agentName": "nordlys" })).is_empty());
        assert!(crossings(&conditions, &json!({ "agentName": " Nordlys" })).is_empty());
        // Fail-closed: absent, null, or not a string.
        assert_eq!(
            crossings(&conditions, &json!({ "message": "quote" })),
            vec!["agentName missing".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "agentName": null })),
            vec!["agentName missing".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "agentName": ["Nordlys"] })),
            vec!["agentName not a string".to_owned()]
        );
        assert_eq!(
            crossings(&conditions, &json!({ "agentName": 7 })),
            vec!["agentName not a string".to_owned()]
        );
        // With a threshold on the same tool: every condition crossed is named, in order.
        let both = vec![
            ApprovalCondition::above("amount", 500.0),
            conditions[0].clone(),
        ];
        assert_eq!(
            crossings(&both, &json!({ "amount": 900, "agentName": "Nordlys" })),
            vec![
                "amount 900 > 500".to_owned(),
                "agentName = \"Nordlys\"".to_owned()
            ]
        );
        assert!(crossings(&both, &json!({ "amount": 9, "agentName": "Other" })).is_empty());
    }

    #[test]
    fn a_condition_without_a_single_test_gates() {
        // Refused when the agent is built (the bridge); should one reach a call, it gates.
        let neither = ApprovalCondition {
            argument: "amount".to_owned(),
            above: None,
            one_of: None,
        };
        let both = ApprovalCondition {
            argument: "amount".to_owned(),
            above: Some(1.0),
            one_of: Some(vec!["1".to_owned()]),
        };
        for condition in [neither, both] {
            assert_eq!(
                crossings(&[condition], &json!({ "amount": 0 })),
                vec!["amount has no single test".to_owned()]
            );
        }
    }

    #[test]
    fn a_condition_reads_and_writes_its_wire_names() {
        let threshold: ApprovalCondition =
            serde_json::from_value(json!({ "argument": "amount", "above": 500 }))
                .expect("a 2.4.0 condition reads");
        assert_eq!(threshold, ApprovalCondition::above("amount", 500.0));
        let named: ApprovalCondition =
            serde_json::from_value(json!({ "argument": "agentName", "in": ["Nordlys"] }))
                .expect("a named-values condition reads");
        assert_eq!(named, ApprovalCondition::one_of("agentName", ["Nordlys"]));
        assert_eq!(
            serde_json::to_value(&named).expect("serializes"),
            json!({ "argument": "agentName", "in": ["Nordlys"] })
        );
        assert_eq!(
            serde_json::to_value(&threshold).expect("serializes"),
            json!({ "argument": "amount", "above": 500.0 })
        );
    }

    #[test]
    fn a_tool_without_conditions_keeps_its_wire_shape() {
        let definition = ToolDefinition {
            name: "refund".to_owned(),
            description: "Refunds an order.".to_owned(),
            requires_approval: true,
            input_schema: None,
            content_scoped: false,
            approval_conditions: Vec::new(),
        };
        let wire = serde_json::to_value(&definition).expect("serializes");
        assert!(wire.get("approvalConditions").is_none());
        let conditioned = ToolDefinition {
            approval_conditions: vec![ApprovalCondition::above("amount", 500.0)],
            ..definition
        };
        let wire = serde_json::to_value(&conditioned).expect("serializes");
        assert_eq!(
            wire["approvalConditions"],
            json!([{ "argument": "amount", "above": 500.0 }])
        );
    }

    #[tokio::test]
    async fn registers_resolves_and_runs_a_sync_tool() {
        let mut registry = InMemoryToolRegistry::new();
        registry.register(
            ToolDefinition {
                name: "echo".to_owned(),
                description: "Echoes its input.".to_owned(),
                requires_approval: false,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
                approval_conditions: Vec::new(),
            },
            sync_tool(Ok),
        );

        assert!(registry.resolve("missing").is_none());
        assert_eq!(registry.list().len(), 1);

        let (definition, handler) = registry.resolve("echo").expect("echo is registered");
        assert!(!definition.requires_approval);
        let output = handler(json!({ "x": 1 })).await;
        assert_eq!(output, Ok(json!({ "x": 1 })));

        // camelCase wire shape, matching the TS model.
        let wire = serde_json::to_string(definition).expect("serializes");
        assert!(wire.contains("\"requiresApproval\":false"));
        assert!(wire.contains("\"inputSchema\""));
    }
}
