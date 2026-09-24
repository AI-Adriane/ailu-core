//! Namespace, key, and stored-value types for the memory store.
//!
//! These mirror the TypeScript `@ailu-ai/memory-store` `types.ts` shapes so the
//! data is wire-compatible across the napi boundary: `MemoryNamespace` is an
//! ordered list of segments, `MemoryKey` is a plain string, and `MemoryItem`
//! carries the stored JSON value plus ISO-8601 timestamps.

use serde::{Deserialize, Serialize};

/// An ordered list of namespace segments, e.g. `["user:u1"]` or
/// `["agent:risk", "session:42"]`. Mirrors the TS `MemoryNamespace = string[]`.
pub type MemoryNamespace = Vec<String>;

/// The key of an item within a namespace. Mirrors the TS `MemoryKey = string`.
pub type MemoryKey = String;

/// A stored memory item: the JSON value plus its namespace, key, and timestamps.
///
/// Field names serialize as camelCase (`createdAt`, `updatedAt`) to stay
/// wire-compatible with the TS `MemoryItem`. `embedding` is omitted from the
/// serialized form when absent, matching the optional `embedding?` field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// The namespace this item belongs to.
    pub namespace: MemoryNamespace,
    /// The key of this item within its namespace.
    pub key: MemoryKey,
    /// The stored value — arbitrary JSON, the Rust analogue of TS `unknown`.
    pub value: serde_json::Value,
    /// ISO-8601 timestamp captured when the item was first written.
    pub created_at: String,
    /// ISO-8601 timestamp captured on the most recent write.
    pub updated_at: String,
    /// Optional embedding vector used by vector-backed stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_timestamps_as_camel_case_and_omits_absent_embedding() {
        let item = MemoryItem {
            namespace: vec!["user:u1".to_owned()],
            key: "profile".to_owned(),
            value: json!({ "name": "Ada" }),
            created_at: "2026-06-11T00:00:00.000Z".to_owned(),
            updated_at: "2026-06-11T00:00:01.000Z".to_owned(),
            embedding: None,
        };
        let serialized = serde_json::to_value(&item).unwrap();
        assert_eq!(
            serialized,
            json!({
                "namespace": ["user:u1"],
                "key": "profile",
                "value": { "name": "Ada" },
                "createdAt": "2026-06-11T00:00:00.000Z",
                "updatedAt": "2026-06-11T00:00:01.000Z"
            })
        );
    }
}
