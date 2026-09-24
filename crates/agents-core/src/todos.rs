//! The `writeTodos` planning tool and its checkpointed state shape — phase 1 of
//! the governed deep-agent harness (ADR 0022/0023).
//!
//! Rust port of `@ailu-ai/agents-core`'s `todos.ts`. `writeTodos` is a *pure*
//! state-write tool the model calls to (re)emit the **authoritative full** todo
//! list. It runs through the same [`crate::tools`] path as every other tool — no
//! new seam, no LLM call. The agent node handler persists the latest list into the
//! reserved [`TODOS_CHANNEL`] in the same checkpointed update as the `AgentResult`,
//! so todos survive across nodes (and across a suspension) without adding a second
//! checkpoint.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::{sync_tool, ToolDefinition, ToolHandler};

/// Reserved channel the agent node persists the latest todo list into. Matches the
/// TS `TODOS_CHANNEL`. Replace semantics: `writeTodos` always writes the full
/// authoritative list, so the channel must not declare an append/merge reducer.
pub const TODOS_CHANNEL: &str = "__todos";

/// The `writeTodos` tool name. Matches the TS `WRITE_TODOS_TOOL_NAME`.
pub const WRITE_TODOS_TOOL: &str = "writeTodos";

/// Lifecycle of a single todo. Serializes snake_case
/// (`pending`/`in_progress`/`completed`), wire-identical to the TS `TodoStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// A single todo. Serializes camelCase (`id`/`text`/`status`), wire-identical to
/// the TS `TodoItem`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub status: TodoStatus,
}

/// Normalize raw tool input into an authoritative todo list. Lenient and
/// deterministic — **byte-for-byte parity** with the TS `normalizeTodos`:
/// - iterate `input.todos` in order;
/// - drop any item whose `text` is missing or blank (after trimming);
/// - an item with a missing or blank `id` gets `todo-{n}`, where `n` is its
///   **1-based position in the incoming list** (dropped items still advance `n`);
/// - an unknown or absent `status` coerces to `pending`.
///
/// A missing/`non-array` `todos` field yields an empty list.
pub fn normalize_todos(input: &Value) -> Vec<TodoItem> {
    let Some(items) = input.get("todos").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, raw) in items.iter().enumerate() {
        let text = raw
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        if text.is_empty() {
            continue;
        }
        let id = match raw.get("id").and_then(Value::as_str) {
            Some(id) if !id.trim().is_empty() => id.to_owned(),
            _ => format!("todo-{}", index + 1),
        };
        let status = match raw.get("status").and_then(Value::as_str) {
            Some("in_progress") => TodoStatus::InProgress,
            Some("completed") => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        };
        out.push(TodoItem { id, text, status });
    }
    out
}

/// The `writeTodos` tool: definition (advertised to the LLM) + a pure handler that
/// returns the normalized list. `requires_approval` is always false — planning is
/// cheap and never gated.
pub fn write_todos_tool() -> (ToolDefinition, ToolHandler) {
    let definition = ToolDefinition {
        name: WRITE_TODOS_TOOL.to_owned(),
        description: "Record or update your plan as a todo list. Always re-emit the COMPLETE \
            authoritative list (every call replaces the previous one). Each item: a short `text` \
            and a `status` of pending, in_progress, or completed."
            .to_owned(),
        requires_approval: false,
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "text": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        },
                        "required": ["text", "status"]
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        })),
        content_scoped: false,
    };
    let handler = sync_tool(|input| {
        let todos = normalize_todos(&input);
        serde_json::to_value(todos).map_err(|error| error.to_string())
    });
    (definition, handler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_one_based_ids_for_missing_or_blank_ids() {
        let todos = normalize_todos(&json!({
            "todos": [
                { "text": "first", "status": "pending" },
                { "id": "  ", "text": "second", "status": "in_progress" },
                { "id": "keep", "text": "third", "status": "completed" }
            ]
        }));
        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0].id, "todo-1");
        assert_eq!(todos[1].id, "todo-2");
        assert_eq!(todos[2].id, "keep");
    }

    #[test]
    fn drops_blank_text_but_keeps_the_one_based_position() {
        // The blank-text row at input index 1 is dropped, but the next row keeps its
        // incoming 1-based position (`todo-3`, not `todo-2`).
        let todos = normalize_todos(&json!({
            "todos": [
                { "text": "first", "status": "pending" },
                { "text": "   ", "status": "pending" },
                { "text": "third", "status": "pending" }
            ]
        }));
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].id, "todo-1");
        assert_eq!(todos[1].id, "todo-3");
    }

    #[test]
    fn coerces_unknown_or_absent_status_to_pending() {
        let todos = normalize_todos(&json!({
            "todos": [
                { "text": "a", "status": "bogus" },
                { "text": "b" }
            ]
        }));
        assert_eq!(todos[0].status, TodoStatus::Pending);
        assert_eq!(todos[1].status, TodoStatus::Pending);
    }

    #[test]
    fn missing_todos_field_yields_empty() {
        assert!(normalize_todos(&json!({})).is_empty());
        assert!(normalize_todos(&json!({ "todos": "nope" })).is_empty());
    }

    #[test]
    fn todo_item_serializes_camel_case_with_snake_case_status() {
        let item = TodoItem {
            id: "todo-1".to_owned(),
            text: "ship it".to_owned(),
            status: TodoStatus::InProgress,
        };
        let wire = serde_json::to_string(&item).expect("serializes");
        assert!(wire.contains("\"status\":\"in_progress\""));
        assert!(wire.contains("\"id\":\"todo-1\""));
        let back: TodoItem = serde_json::from_str(&wire).expect("round-trips");
        assert_eq!(back, item);
    }

    #[tokio::test]
    async fn the_tool_normalizes_and_is_not_gated() {
        let (definition, handler) = write_todos_tool();
        assert_eq!(definition.name, WRITE_TODOS_TOOL);
        assert!(!definition.requires_approval);
        assert!(definition.input_schema.is_some());

        let output = handler(json!({
            "todos": [{ "text": "do thing", "status": "pending" }]
        }))
        .await
        .expect("runs");
        let todos: Vec<TodoItem> = serde_json::from_value(output).expect("parses");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, "todo-1");
    }
}
