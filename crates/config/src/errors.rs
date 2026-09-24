//! Configuration errors — the Rust port of `@ailu-ai/config`'s `errors.ts`.
//!
//! The TS layer throws a single `ConfigValidationError` whose `issues` array
//! aggregates every Zod issue from `safeParse`. We mirror that behaviour: each
//! individual failure is a [`ConfigIssue`] (carrying the offending key plus a
//! human-readable message), and [`ConfigError::Validation`] aggregates all the
//! issues collected during a single `parse_env` pass.

use thiserror::Error;

/// A single environment-validation issue, mirroring one Zod issue.
///
/// `key` is the environment variable name that failed (the Zod `path`); the TS
/// model also distinguishes missing-vs-invalid through the issue message, which
/// we reproduce via [`ConfigIssueKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigIssue {
    /// The environment variable name that failed validation.
    pub key: String,
    /// The kind of failure (missing required, invalid number, etc.).
    pub kind: ConfigIssueKind,
    /// A human-readable description of the failure.
    pub message: String,
}

impl ConfigIssue {
    /// Build a "required key missing" issue.
    pub fn missing(key: &str) -> Self {
        ConfigIssue {
            key: key.to_owned(),
            kind: ConfigIssueKind::MissingRequired,
            message: format!("Required environment variable `{key}` is missing."),
        }
    }

    /// Build an "invalid enum value" issue.
    pub fn invalid_enum(key: &str, value: &str, allowed: &[&str]) -> Self {
        ConfigIssue {
            key: key.to_owned(),
            kind: ConfigIssueKind::InvalidEnum,
            message: format!(
                "Invalid value `{value}` for `{key}`; expected one of: {}.",
                allowed.join(", ")
            ),
        }
    }

    /// Build an "invalid number" issue.
    pub fn invalid_number(key: &str, value: &str) -> Self {
        ConfigIssue {
            key: key.to_owned(),
            kind: ConfigIssueKind::InvalidNumber,
            message: format!("Invalid value `{value}` for `{key}`; expected a positive integer."),
        }
    }

    /// Build an "empty string" issue (Zod `min(1)` failure on a present value).
    pub fn empty_string(key: &str) -> Self {
        ConfigIssue {
            key: key.to_owned(),
            kind: ConfigIssueKind::EmptyString,
            message: format!("Environment variable `{key}` must not be empty."),
        }
    }
}

/// The category of a configuration issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigIssueKind {
    /// A required key was absent from the source map.
    MissingRequired,
    /// A value did not match the allowed enum variants.
    InvalidEnum,
    /// A value could not be parsed as a positive integer.
    InvalidNumber,
    /// A present value violated the non-empty (`min(1)`) constraint.
    EmptyString,
}

/// Errors raised while parsing environment configuration.
///
/// Mirrors `ConfigValidationError` from the TS package: a single aggregate
/// error whose `issues` carry the per-key failures.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// One or more environment variables failed validation.
    #[error("Invalid environment configuration.")]
    Validation {
        /// Every issue collected during the parse pass.
        issues: Vec<ConfigIssue>,
    },
}

impl ConfigError {
    /// Construct a validation error from a list of issues.
    pub fn validation(issues: Vec<ConfigIssue>) -> Self {
        ConfigError::Validation { issues }
    }

    /// Borrow the collected issues.
    pub fn issues(&self) -> &[ConfigIssue] {
        match self {
            ConfigError::Validation { issues } => issues,
        }
    }
}
