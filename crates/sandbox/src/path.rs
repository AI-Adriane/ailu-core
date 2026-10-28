//! Logical paths inside a sandbox, validated by the same rule as the virtual filesystem.

use ailu_fs_backend::{normalize_path, FsError};
use serde::{Deserialize, Serialize};

/// A path rejected by normalization. Carries a short reason, never the offending input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("invalid path: {reason}")]
pub struct InvalidPath {
    pub reason: String,
}

/// A normalized, forward-slash, relative path.
///
/// Built only through [`LogicalPath::new`] (or deserialization, which goes through it), so a
/// value of this type never contains `..`, never starts with `/`, and has no backslash or
/// null byte.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LogicalPath(String);

impl LogicalPath {
    /// Validate and normalize `raw` with [`ailu_fs_backend::normalize_path`].
    pub fn new(raw: &str) -> Result<Self, InvalidPath> {
        match normalize_path(raw) {
            Ok(path) => Ok(Self(path)),
            Err(FsError::InvalidPath { reason }) => Err(InvalidPath { reason }),
            // `normalize_path` only fails with `InvalidPath`; stay fail-closed regardless.
            Err(_) => Err(InvalidPath {
                reason: "path rejected".to_owned(),
            }),
        }
    }

    /// Borrow the normalized path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LogicalPath {
    type Error = InvalidPath;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<LogicalPath> for String {
    fn from(value: LogicalPath) -> Self {
        value.0
    }
}

impl std::fmt::Display for LogicalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_normalizes_relative_paths() {
        assert_eq!(LogicalPath::new("a/./b//c").unwrap().as_str(), "a/b/c");
        assert_eq!(LogicalPath::new("./work").unwrap().as_str(), "work");
    }

    #[test]
    fn rejects_what_the_filesystem_rejects() {
        for bad in ["../x", "a/../../b", "/abs", "a\\b", "a\0b", "", "///"] {
            assert!(LogicalPath::new(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn reason_never_echoes_the_input() {
        let err = LogicalPath::new("secret/../x").unwrap_err();
        assert!(!err.reason.contains("secret"));
    }

    #[test]
    fn deserialization_validates() {
        let ok: LogicalPath = serde_json::from_str("\"a//b\"").unwrap();
        assert_eq!(ok.as_str(), "a/b");
        assert!(serde_json::from_str::<LogicalPath>("\"../b\"").is_err());
        assert_eq!(serde_json::to_string(&ok).unwrap(), "\"a/b\"");
    }
}
