//! Artifact data model — the Rust port of `@ailu-ai/artifact-store`'s `types.ts`.
//!
//! These types serialize to the exact same wire shape as the TypeScript model
//! (camelCase fields, branded ids carried as plain strings) so that an
//! [`Artifact`] or [`ArtifactRef`] produced here is interchangeable with one
//! produced by the TS engine across an API boundary.

use ailu_graph_core::{NodeId, RunId};
use serde::{Deserialize, Serialize};

/// Branded identifier of a logical artifact (a `runId:name` pair in the
/// in-memory store). Serializes transparently as a plain string, matching the
/// TS `ArtifactId = string & { __brand: "ArtifactId" }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ArtifactId {
    fn from(value: &str) -> Self {
        ArtifactId(value.to_owned())
    }
}

impl From<String> for ArtifactId {
    fn from(value: String) -> Self {
        ArtifactId(value)
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Monotonically increasing version of an artifact. The first write is `1` and
/// each subsequent write of the same `runId:name` increments by one (mirrors
/// the TS `ArtifactVersion = number`, constrained to `int >= 1`).
pub type ArtifactVersion = i64;

/// Closed set of supported artifact media types, matching the TS
/// `ARTIFACT_MEDIA_TYPES` tuple. Serializes as the exact MIME strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtifactMediaType {
    /// `application/json`
    #[serde(rename = "application/json")]
    ApplicationJson,
    /// `text/plain`
    #[serde(rename = "text/plain")]
    TextPlain,
    /// `text/markdown`
    #[serde(rename = "text/markdown")]
    TextMarkdown,
    /// `application/octet-stream`
    #[serde(rename = "application/octet-stream")]
    ApplicationOctetStream,
}

/// A single, immutable, versioned artifact produced by a node during a run.
///
/// Field names serialize in camelCase to match the TS `Artifact` wire shape.
/// `content` is opaque ([`serde_json::Value`]) — the equivalent of the TS
/// `unknown` — and `metadata` is an optional free-form map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Logical artifact id (stable across versions).
    pub id: ArtifactId,
    /// Run that produced this artifact.
    pub run_id: RunId,
    /// Node that produced this artifact.
    pub node_id: NodeId,
    /// Human-readable name, unique per run.
    pub name: String,
    /// Media type of the content.
    pub media_type: ArtifactMediaType,
    /// Version number (`1`-based, increments on each write of the same name).
    pub version: ArtifactVersion,
    /// Opaque content payload.
    pub content: serde_json::Value,
    /// RFC 3339 creation timestamp, stamped at write time.
    pub created_at: String,
    /// Optional free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

/// A pointer to a specific version of an artifact: the `id`/`version` pair
/// returned by a write and consumed by version-specific reads. Mirrors the TS
/// `ArtifactRef` exactly (camelCase wire shape).
///
/// This type is referenced by other crates, so its shape must stay stable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    /// Logical artifact id.
    pub id: ArtifactId,
    /// Targeted version.
    pub version: ArtifactVersion,
}

impl Artifact {
    /// Build an [`ArtifactRef`] pointing at this artifact's exact version.
    pub fn as_ref(&self) -> ArtifactRef {
        ArtifactRef {
            id: self.id.clone(),
            version: self.version,
        }
    }
}

/// The data a caller supplies to [`crate::ArtifactStore::write`]: everything
/// except the store-managed `id`, `version`, and `createdAt`. This is the Rust
/// equivalent of the TS `Omit<Artifact, "id" | "version" | "createdAt">`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactWriteInput {
    /// Run that produced this artifact.
    pub run_id: RunId,
    /// Node that produced this artifact.
    pub node_id: NodeId,
    /// Human-readable name, unique per run.
    pub name: String,
    /// Media type of the content.
    pub media_type: ArtifactMediaType,
    /// Opaque content payload.
    pub content: serde_json::Value,
    /// Optional free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_id_serializes_transparently_as_a_string() {
        let id = ArtifactId::from("run-1:analysis");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"run-1:analysis\"");
        let back: ArtifactId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn media_type_serializes_as_the_mime_string() {
        let json = serde_json::to_string(&ArtifactMediaType::ApplicationJson).unwrap();
        assert_eq!(json, "\"application/json\"");
        let back: ArtifactMediaType = serde_json::from_str("\"text/markdown\"").unwrap();
        assert_eq!(back, ArtifactMediaType::TextMarkdown);
    }

    #[test]
    fn artifact_serializes_with_camelcase_keys() {
        let artifact = Artifact {
            id: ArtifactId::from("run-1:analysis"),
            run_id: RunId::from("run-1"),
            node_id: NodeId::from("node-1"),
            name: "analysis".to_owned(),
            media_type: ArtifactMediaType::ApplicationJson,
            version: 1,
            content: serde_json::json!({ "score": 1 }),
            created_at: "2026-06-09T00:00:00Z".to_owned(),
            metadata: None,
        };
        let value = serde_json::to_value(&artifact).unwrap();
        assert!(value.get("runId").is_some());
        assert!(value.get("nodeId").is_some());
        assert!(value.get("mediaType").is_some());
        assert!(value.get("createdAt").is_some());
        // `metadata` is omitted when absent, matching the optional TS field.
        assert!(value.get("metadata").is_none());
    }

    #[test]
    fn artifact_ref_round_trips_through_camelcase_json() {
        let reference = ArtifactRef {
            id: ArtifactId::from("run-1:analysis"),
            version: 3,
        };
        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(json, r#"{"id":"run-1:analysis","version":3}"#);
        let back: ArtifactRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reference);
    }
}
