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
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical_json(input).to_string().as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{name}#{hex}")
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
