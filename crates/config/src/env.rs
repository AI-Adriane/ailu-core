//! Typed environment configuration — the Rust port of `@adriane-ai/config`'s
//! `env.ts`.
//!
//! The TS layer defines a Zod `EnvironmentSchema` and exposes `parseEnv(source)`
//! (validates a map, throwing `ConfigValidationError` on failure) plus a cached
//! `getEnv()` reading `process.env`. We mirror that shape:
//!
//! - [`parse_env`] is the pure function: it validates a `HashMap<String, String>`
//!   and returns `Result<Env, ConfigError>`, collecting *all* failures into a
//!   single aggregate error just like Zod's `safeParse`.
//! - [`get_env`] reads the real process environment via [`std::env`] and caches
//!   the parsed result, mirroring the `cachedEnv` memoisation in TS.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::errors::{ConfigError, ConfigIssue};

/// Deployment environment — mirrors the `NODE_ENV` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeEnv {
    /// Local development.
    Local,
    /// Staging / pre-production.
    Staging,
    /// Production.
    Production,
}

impl NodeEnv {
    const ALLOWED: [&'static str; 3] = ["local", "staging", "production"];

    fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(NodeEnv::Local),
            "staging" => Some(NodeEnv::Staging),
            "production" => Some(NodeEnv::Production),
            _ => None,
        }
    }
}

/// Log verbosity — mirrors the `LOG_LEVEL` enum (default `info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    /// Debug-level logging.
    Debug,
    /// Informational logging (the default).
    Info,
    /// Warnings only.
    Warn,
    /// Errors only.
    Error,
}

impl LogLevel {
    const ALLOWED: [&'static str; 4] = ["debug", "info", "warn", "error"];

    fn parse(value: &str) -> Option<Self> {
        match value {
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

/// Validated application environment — mirrors the inferred `AppEnv` type.
#[derive(Clone, PartialEq, Eq)]
pub struct Env {
    /// `NODE_ENV` — required.
    pub node_env: NodeEnv,
    /// `PORT` — positive integer, default `3000`.
    pub port: u16,
    /// `DATABASE_URL` — required, non-empty.
    pub database_url: String,
    /// `REDIS_URL` — required, non-empty.
    pub redis_url: String,
    /// `JWT_SECRET` — required, non-empty.
    pub jwt_secret: String,
    /// `JWT_EXPIRY` — non-empty, default `"1h"`.
    pub jwt_expiry: String,
    /// `OPENAI_API_KEY` — optional, non-empty when present.
    pub openai_api_key: Option<String>,
    /// `ANTHROPIC_API_KEY` — optional, non-empty when present.
    pub anthropic_api_key: Option<String>,
    /// `MISTRAL_API_KEY` — optional, non-empty when present.
    pub mistral_api_key: Option<String>,
    /// `OTEL_ENDPOINT` — optional, non-empty when present.
    pub otel_endpoint: Option<String>,
    /// `LOG_LEVEL` — default `info`.
    pub log_level: LogLevel,
}

/// Secrets (the JWT secret, provider keys) and connection URLs (which usually embed a password)
/// are redacted, so logging an `Env` with `{:?}` never writes a credential.
impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const REDACTED: &str = "<redacted>";
        let optional = |value: &Option<String>| value.as_ref().map(|_| REDACTED);
        f.debug_struct("Env")
            .field("node_env", &self.node_env)
            .field("port", &self.port)
            .field("database_url", &REDACTED)
            .field("redis_url", &REDACTED)
            .field("jwt_secret", &REDACTED)
            .field("jwt_expiry", &self.jwt_expiry)
            .field("openai_api_key", &optional(&self.openai_api_key))
            .field("anthropic_api_key", &optional(&self.anthropic_api_key))
            .field("mistral_api_key", &optional(&self.mistral_api_key))
            .field("otel_endpoint", &self.otel_endpoint)
            .field("log_level", &self.log_level)
            .finish()
    }
}

/// Validate a source map into a typed [`Env`].
///
/// Pure and side-effect free, so tests inject a map instead of touching the
/// process environment — mirroring `parseEnv(source)` in TS. Every validation
/// failure is collected; if any issue is found the function returns a single
/// [`ConfigError::Validation`] carrying all of them, matching Zod `safeParse`.
pub fn parse_env(source: &HashMap<String, String>) -> Result<Env, ConfigError> {
    let mut issues: Vec<ConfigIssue> = Vec::new();

    // NODE_ENV — required enum.
    let node_env = match source.get("NODE_ENV") {
        None => {
            issues.push(ConfigIssue::missing("NODE_ENV"));
            None
        }
        Some(raw) => match NodeEnv::parse(raw) {
            Some(v) => Some(v),
            None => {
                issues.push(ConfigIssue::invalid_enum(
                    "NODE_ENV",
                    raw,
                    &NodeEnv::ALLOWED,
                ));
                None
            }
        },
    };

    // PORT — coerced positive integer, default 3000.
    let port = match source.get("PORT") {
        None => Some(3000u16),
        Some(raw) => match parse_port(raw) {
            Some(v) => Some(v),
            None => {
                issues.push(ConfigIssue::invalid_number("PORT", raw));
                None
            }
        },
    };

    let database_url = required_non_empty(source, "DATABASE_URL", &mut issues);
    let redis_url = required_non_empty(source, "REDIS_URL", &mut issues);
    let jwt_secret = required_non_empty(source, "JWT_SECRET", &mut issues);

    // JWT_EXPIRY — non-empty, default "1h".
    let jwt_expiry = match source.get("JWT_EXPIRY") {
        None => Some("1h".to_owned()),
        Some(raw) if raw.is_empty() => {
            issues.push(ConfigIssue::empty_string("JWT_EXPIRY"));
            None
        }
        Some(raw) => Some(raw.clone()),
    };

    let openai_api_key = optional_non_empty(source, "OPENAI_API_KEY", &mut issues);
    let anthropic_api_key = optional_non_empty(source, "ANTHROPIC_API_KEY", &mut issues);
    let mistral_api_key = optional_non_empty(source, "MISTRAL_API_KEY", &mut issues);
    let otel_endpoint = optional_non_empty(source, "OTEL_ENDPOINT", &mut issues);

    // LOG_LEVEL — enum, default "info".
    let log_level = match source.get("LOG_LEVEL") {
        None => Some(LogLevel::Info),
        Some(raw) => match LogLevel::parse(raw) {
            Some(v) => Some(v),
            None => {
                issues.push(ConfigIssue::invalid_enum(
                    "LOG_LEVEL",
                    raw,
                    &LogLevel::ALLOWED,
                ));
                None
            }
        },
    };

    if !issues.is_empty() {
        return Err(ConfigError::validation(issues));
    }

    // Every field is `Some` here: each was populated unless an issue was pushed,
    // and we returned above if any issue exists.
    Ok(Env {
        node_env: node_env.expect("validated"),
        port: port.expect("validated"),
        database_url: database_url.expect("validated"),
        redis_url: redis_url.expect("validated"),
        jwt_secret: jwt_secret.expect("validated"),
        jwt_expiry: jwt_expiry.expect("validated"),
        openai_api_key,
        anthropic_api_key,
        mistral_api_key,
        otel_endpoint,
        log_level: log_level.expect("validated"),
    })
}

/// Read and validate the real process environment, caching the result.
///
/// Mirrors `getEnv()`: the first call parses `std::env::vars` and memoises a
/// successful [`Env`]; subsequent calls return the cached value. A failed parse
/// is *not* cached, so a later call after fixing the environment can succeed.
pub fn get_env() -> Result<Env, ConfigError> {
    static CACHE: OnceLock<Env> = OnceLock::new();
    if let Some(env) = CACHE.get() {
        return Ok(env.clone());
    }
    let source: HashMap<String, String> = std::env::vars().collect();
    let env = parse_env(&source)?;
    // If another thread won the race, keep the existing value; both are valid.
    Ok(CACHE.get_or_init(|| env).clone())
}

/// Resolve a required, non-empty string field, pushing an issue on failure.
fn required_non_empty(
    source: &HashMap<String, String>,
    key: &str,
    issues: &mut Vec<ConfigIssue>,
) -> Option<String> {
    match source.get(key) {
        None => {
            issues.push(ConfigIssue::missing(key));
            None
        }
        Some(raw) if raw.is_empty() => {
            issues.push(ConfigIssue::empty_string(key));
            None
        }
        Some(raw) => Some(raw.clone()),
    }
}

/// Resolve an optional string field; absent is `None`, but a present empty
/// value violates the `min(1)` constraint and pushes an issue.
fn optional_non_empty(
    source: &HashMap<String, String>,
    key: &str,
    issues: &mut Vec<ConfigIssue>,
) -> Option<String> {
    match source.get(key) {
        None => None,
        Some(raw) if raw.is_empty() => {
            issues.push(ConfigIssue::empty_string(key));
            None
        }
        Some(raw) => Some(raw.clone()),
    }
}

/// Parse a port value: a positive integer that fits the valid port range.
///
/// Mirrors Zod `z.coerce.number().int().positive()`: rejects non-numeric,
/// non-integer, zero, and negative values.
fn parse_port(raw: &str) -> Option<u16> {
    let trimmed = raw.trim();
    // Reject empty (Zod coerces "" to 0, which then fails `.positive()`).
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u32>() {
        Ok(0) => None,
        Ok(n) if n <= u16::MAX as u32 => Some(n as u16),
        // Positive but out of TCP port range — still an invalid port.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ConfigIssueKind;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn parses_a_valid_environment() {
        let env = parse_env(&map(&[
            ("NODE_ENV", "local"),
            ("PORT", "4000"),
            ("DATABASE_URL", "postgres://localhost:5432/adriane"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
            ("JWT_EXPIRY", "2h"),
            ("LOG_LEVEL", "debug"),
        ]))
        .expect("valid env should parse");

        assert_eq!(env.port, 4000);
        assert_eq!(env.node_env, NodeEnv::Local);
        assert_eq!(env.log_level, LogLevel::Debug);
        assert_eq!(env.jwt_expiry, "2h");
    }

    #[test]
    fn debug_output_never_contains_a_credential() {
        let env = parse_env(&map(&[
            ("NODE_ENV", "local"),
            (
                "DATABASE_URL",
                "postgres://app:db-password@localhost:5432/adriane",
            ),
            ("REDIS_URL", "redis://:redis-password@localhost:6379"),
            ("JWT_SECRET", "jwt-signing-secret"),
            ("OPENAI_API_KEY", "sk-openai-secret"),
        ]))
        .expect("valid env should parse");

        let rendered = format!("{env:?}");
        for secret in [
            "db-password",
            "redis-password",
            "jwt-signing-secret",
            "sk-openai-secret",
        ] {
            assert!(!rendered.contains(secret), "{secret} leaked: {rendered}");
        }
        assert!(rendered.contains("port: 3000"), "{rendered}");
    }

    #[test]
    fn throws_when_required_variable_is_missing() {
        // Missing DATABASE_URL (and PORT/JWT_EXPIRY/LOG_LEVEL have defaults).
        let result = parse_env(&map(&[
            ("NODE_ENV", "local"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
            ("LOG_LEVEL", "info"),
        ]));

        let err = result.expect_err("missing required key must error");
        let issues = err.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "DATABASE_URL");
        assert_eq!(issues[0].kind, ConfigIssueKind::MissingRequired);
    }

    #[test]
    fn applies_default_values() {
        let env = parse_env(&map(&[
            ("NODE_ENV", "staging"),
            ("DATABASE_URL", "postgres://localhost:5432/adriane"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
        ]))
        .expect("env with defaults should parse");

        assert_eq!(env.port, 3000);
        assert_eq!(env.jwt_expiry, "1h");
        assert_eq!(env.log_level, LogLevel::Info);
        assert_eq!(env.node_env, NodeEnv::Staging);
    }

    #[test]
    fn rejects_invalid_enum_value() {
        let result = parse_env(&map(&[
            ("NODE_ENV", "dev"),
            ("DATABASE_URL", "postgres://localhost:5432/adriane"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
        ]));

        let err = result.expect_err("invalid enum must error");
        let issues = err.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "NODE_ENV");
        assert_eq!(issues[0].kind, ConfigIssueKind::InvalidEnum);
    }

    #[test]
    fn rejects_invalid_and_non_positive_port() {
        for bad in ["abc", "0", "-5", "  "] {
            let result = parse_env(&map(&[
                ("NODE_ENV", "production"),
                ("PORT", bad),
                ("DATABASE_URL", "postgres://localhost:5432/adriane"),
                ("REDIS_URL", "redis://localhost:6379"),
                ("JWT_SECRET", "super-secret"),
            ]));
            let err = result.expect_err("invalid port must error");
            let issues = err.issues();
            assert_eq!(issues.len(), 1, "PORT={bad}");
            assert_eq!(issues[0].key, "PORT");
            assert_eq!(issues[0].kind, ConfigIssueKind::InvalidNumber);
        }
    }

    #[test]
    fn aggregates_all_issues_in_one_error() {
        // Missing NODE_ENV, DATABASE_URL, REDIS_URL, JWT_SECRET => 4 issues.
        let result = parse_env(&HashMap::new());
        let err = result.expect_err("empty map must error");
        let keys: Vec<&str> = err.issues().iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"NODE_ENV"));
        assert!(keys.contains(&"DATABASE_URL"));
        assert!(keys.contains(&"REDIS_URL"));
        assert!(keys.contains(&"JWT_SECRET"));
        assert_eq!(err.issues().len(), 4);
    }

    #[test]
    fn optional_keys_are_none_when_absent_and_set_when_present() {
        let env = parse_env(&map(&[
            ("NODE_ENV", "local"),
            ("DATABASE_URL", "postgres://localhost:5432/adriane"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
            ("ANTHROPIC_API_KEY", "sk-ant-123"),
        ]))
        .expect("valid env should parse");

        assert_eq!(env.anthropic_api_key.as_deref(), Some("sk-ant-123"));
        assert_eq!(env.openai_api_key, None);
        assert_eq!(env.mistral_api_key, None);
        assert_eq!(env.otel_endpoint, None);
    }

    #[test]
    fn rejects_empty_present_optional_value() {
        let result = parse_env(&map(&[
            ("NODE_ENV", "local"),
            ("DATABASE_URL", "postgres://localhost:5432/adriane"),
            ("REDIS_URL", "redis://localhost:6379"),
            ("JWT_SECRET", "super-secret"),
            ("OPENAI_API_KEY", ""),
        ]));
        let err = result.expect_err("empty optional must error");
        let issues = err.issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "OPENAI_API_KEY");
        assert_eq!(issues[0].kind, ConfigIssueKind::EmptyString);
    }
}
