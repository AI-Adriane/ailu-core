//! Rust port of `@ailu-ai/config` — typed environment parsing and feature
//! flags.
//!
//! Two surfaces, mirroring the TS package:
//!
//! - [`parse_env`] / [`get_env`] validate the deployment environment into a
//!   typed [`Env`], raising an aggregate [`ConfigError`] on failure.
//! - [`is_enabled`] / [`get_all_flags`] read env-driven [`FeatureFlag`]s.
//!
//! `parse_env` is pure (it takes an injected `HashMap`), so callers and tests
//! never need to touch the real process environment.

#![forbid(unsafe_code)]

mod env;
mod errors;
mod feature_flags;

pub use env::{get_env, parse_env, Env, LogLevel, NodeEnv};
pub use errors::{ConfigError, ConfigIssue, ConfigIssueKind};
pub use feature_flags::{get_all_flags, is_enabled, FeatureFlag, FEATURE_FLAGS};
