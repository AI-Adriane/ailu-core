//! Wire types for the governed virtual filesystem seam (ADR 0024). All camelCase,
//! mirrored 1:1 by the TS `@ailu/fs-backend` package.

use ailu_artifact_store::{ArtifactMediaType, ArtifactVersion};
use ailu_graph_core::NodeId;
use serde::{Deserialize, Serialize};

/// A file's content as returned by a read. `content` is the UTF-8 text for
/// `text/plain` / `text/markdown`, the JSON text for `application/json`, and
/// base64 for `application/octet-stream` (the caller interprets it by `media_type`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub media_type: ArtifactMediaType,
    pub version: ArtifactVersion,
    pub created_at: String,
}

/// One entry in a directory listing. A `path` with `is_dir: true` is a SYNTHETIC
/// directory (a prefix under which at least one file lives — the flat artifact
/// keyspace has no real directories; see ADR 0024).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub is_dir: bool,
    /// Latest version for a file; `None` for a synthetic directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<ArtifactVersion>,
}

/// A line-based edit. Line numbers are **1-indexed and inclusive**. Serializes
/// with an `op` tag (`replace` / `insert` / `delete`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditOp {
    /// Replace lines `start_line..=end_line` with `text` (which may be multi-line).
    Replace {
        start_line: usize,
        end_line: usize,
        text: String,
    },
    /// Insert `text` AFTER `after_line` (`after_line: 0` prepends to the file).
    Insert { after_line: usize, text: String },
    /// Delete lines `start_line..=end_line`.
    Delete { start_line: usize, end_line: usize },
}

/// A `grep` hit: the matching line and where it lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line_text: String,
}

/// Attribution threaded into a write/edit/delete/rename. The `run_id` is owned by
/// the (run-scoped) backend; this carries the acting `node_id` (recorded by the
/// artifact store) and the `principal` (who acted — added to `Artifact.metadata`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsWriteCtx {
    pub node_id: NodeId,
    pub principal: Option<String>,
}

/// A per-path permission verb (ADR 0024). Serializes snake_case. Ordered by
/// restrictiveness for fail-closed tie-breaking (see [`FsPermVerb::rank`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsPermVerb {
    /// Invisible: no read, no list, no write. Existence is not disclosed.
    Deny,
    /// Read / ls / glob / grep only.
    Read,
    /// Read + write/edit, but the write routes through an approval gate (phase 2c).
    Gate,
    /// Read + ungated write/edit/delete/rename.
    Write,
}

impl FsPermVerb {
    /// Restrictiveness rank (higher = fewer capabilities = more restrictive):
    /// `deny` > `read` > `gate` > `write`. Used to break a specificity tie
    /// fail-closed (the most restrictive matching rule wins).
    pub fn rank(self) -> u8 {
        match self {
            FsPermVerb::Deny => 3,
            FsPermVerb::Read => 2,
            FsPermVerb::Gate => 1,
            FsPermVerb::Write => 0,
        }
    }

    /// Whether this verb permits reading (everything except `deny`).
    pub fn can_read(self) -> bool {
        !matches!(self, FsPermVerb::Deny)
    }

    /// Whether this verb permits writing (`write` ungated, `gate` via approval).
    pub fn can_write(self) -> bool {
        matches!(self, FsPermVerb::Write | FsPermVerb::Gate)
    }
}

/// Errors from the filesystem seam. Returned to the agent loop as a tool error
/// string (via `Display`); also serializable for the HTTP backend wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FsError {
    /// No file at this path (also the response for a `deny` path — deny never
    /// discloses existence, so it is reported as not-found, not permission-denied).
    #[error("file not found: {path}")]
    NotFound { path: String },
    /// The path's policy forbids this operation (a non-deny denial — e.g. a write
    /// to a `read` path).
    #[error("permission denied for {action} on {path}")]
    PermissionDenied { action: String, path: String },
    /// The path failed normalization (traversal, absolute, null byte, …).
    #[error("invalid path: {reason}")]
    InvalidPath { reason: String },
    /// An edit referenced an out-of-range or inconsistent line range.
    #[error("invalid edit: {reason}")]
    InvalidEdit { reason: String },
    /// The backend does not support this operation (e.g. the Noop backend).
    #[error("operation not supported")]
    NotSupported,
    /// An external backend was unreachable (fail-closed — never a silent pass).
    #[error("filesystem backend unavailable: {reason}")]
    ServiceUnavailable { reason: String },
    /// A lower-level backend failure.
    #[error("backend error: {reason}")]
    Backend { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perm_verb_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&FsPermVerb::Gate).unwrap(),
            "\"gate\""
        );
        let back: FsPermVerb = serde_json::from_str("\"deny\"").unwrap();
        assert_eq!(back, FsPermVerb::Deny);
    }

    #[test]
    fn restrictiveness_order_is_fail_closed() {
        // deny is the most restrictive, write the least.
        assert!(FsPermVerb::Deny.rank() > FsPermVerb::Read.rank());
        assert!(FsPermVerb::Read.rank() > FsPermVerb::Gate.rank());
        assert!(FsPermVerb::Gate.rank() > FsPermVerb::Write.rank());
        assert!(FsPermVerb::Read.can_read() && !FsPermVerb::Read.can_write());
        assert!(FsPermVerb::Gate.can_write() && FsPermVerb::Write.can_write());
        assert!(!FsPermVerb::Deny.can_read());
    }

    #[test]
    fn edit_op_serializes_with_op_tag() {
        let op = EditOp::Replace {
            start_line: 2,
            end_line: 3,
            text: "x".to_owned(),
        };
        let wire = serde_json::to_string(&op).unwrap();
        assert!(wire.contains("\"op\":\"replace\""));
        assert!(wire.contains("\"startLine\":2"));
    }

    #[test]
    fn fs_error_displays_and_serializes_tagged() {
        let err = FsError::NotFound {
            path: "a/b".to_owned(),
        };
        assert_eq!(err.to_string(), "file not found: a/b");
        let wire = serde_json::to_string(&err).unwrap();
        assert!(wire.contains("\"kind\":\"notFound\""));
    }
}
