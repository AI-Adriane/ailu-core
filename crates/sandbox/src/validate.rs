//! Pure validation of requests against deployment-set limits and bounds.
//!
//! Every function is fail-closed, has no side effect, and returns a typed
//! [`SandboxError::PolicyDenied`] (or [`SandboxError::InvalidPath`] for a bad path). Error
//! reasons carry identifiers and numbers, never argument or stdin content.

use crate::error::{BoundKind, PolicyReason, SandboxError};
use crate::ids::SandboxContext;
use crate::limits::{Limits, RequestBounds};
use crate::request::{CollectRequest, CreateRequest, ExecRequest, NetworkPolicy};
use ailu_fs_backend::normalize_path;
use std::collections::HashSet;

/// Longest user-supplied string echoed back in a refusal reason.
const ECHO_MAX_CHARS: usize = 64;

fn denied(reason: PolicyReason) -> SandboxError {
    SandboxError::PolicyDenied(reason)
}

fn echo(value: &str) -> String {
    value.chars().take(ECHO_MAX_CHARS).collect()
}

fn too_large(bound: BoundKind, actual: u64, max: u64) -> SandboxError {
    denied(PolicyReason::RequestTooLarge { bound, actual, max })
}

/// The context must name a tenant, a run and a node, and the attempt is 1-based.
pub fn validate_context(ctx: &SandboxContext) -> Result<(), SandboxError> {
    if ctx.tenant_id.as_str().is_empty()
        || ctx.run_id.as_str().is_empty()
        || ctx.node_id.as_str().is_empty()
        || ctx.attempt == 0
    {
        return Err(denied(PolicyReason::InvalidContext));
    }
    Ok(())
}

/// Validate a `create` request: limits (non-zero, not above `ceiling`), `ttl`, inputs,
/// network rules and environment grants.
pub fn validate_create(
    req: &CreateRequest,
    bounds: &RequestBounds,
    ceiling: &Limits,
) -> Result<(), SandboxError> {
    if let Some(field) = req.limits.first_zero() {
        return Err(denied(PolicyReason::LimitZero { field }));
    }
    if let Some(field) = req.limits.first_above(ceiling) {
        return Err(denied(PolicyReason::LimitAboveCeiling { field }));
    }
    if req.ttl_ms > bounds.max_ttl_ms {
        return Err(too_large(BoundKind::Ttl, req.ttl_ms, bounds.max_ttl_ms));
    }
    if req.inputs.len() as u64 > u64::from(bounds.max_input_files) {
        return Err(denied(PolicyReason::InputTooLarge {
            bound: BoundKind::InputFiles,
            actual: req.inputs.len() as u64,
            max: u64::from(bounds.max_input_files),
        }));
    }
    if let NetworkPolicy::Allow(rules) = &req.network {
        for rule in rules {
            if !is_exact_host(&rule.host) || rule.port == 0 {
                return Err(denied(PolicyReason::NetworkRule {
                    host: echo(&rule.host),
                }));
            }
        }
    }
    let mut seen = HashSet::new();
    for grant in &req.env {
        if !is_env_name(&grant.name) || !seen.insert(grant.name.as_str()) {
            return Err(denied(PolicyReason::InvalidEnvName {
                name: echo(&grant.name),
            }));
        }
    }
    Ok(())
}

/// Validate an `exec` request: non-empty `argv` within count and byte bounds, no null byte,
/// `stdin` within bound. The working directory is a [`crate::LogicalPath`], already valid.
pub fn validate_exec(req: &ExecRequest, bounds: &RequestBounds) -> Result<(), SandboxError> {
    if req.argv.is_empty() {
        return Err(denied(PolicyReason::EmptyArgv));
    }
    let count = req.argv.len() as u64;
    if count > u64::from(bounds.max_argv_count) {
        return Err(too_large(
            BoundKind::ArgvCount,
            count,
            u64::from(bounds.max_argv_count),
        ));
    }
    let bytes: u64 = req.argv.iter().map(|a| a.len() as u64).sum();
    if bytes > u64::from(bounds.max_argv_bytes) {
        return Err(too_large(
            BoundKind::ArgvBytes,
            bytes,
            u64::from(bounds.max_argv_bytes),
        ));
    }
    if req.argv.iter().any(|a| a.contains('\0')) {
        return Err(denied(PolicyReason::NulInArgument));
    }
    if req.argv[0].is_empty() {
        return Err(denied(PolicyReason::EmptyProgram));
    }
    let stdin = req.stdin.as_ref().map_or(0, |s| s.len() as u64);
    if stdin > u64::from(bounds.max_stdin_bytes) {
        return Err(too_large(
            BoundKind::StdinBytes,
            stdin,
            u64::from(bounds.max_stdin_bytes),
        ));
    }
    Ok(())
}

/// Validate a `collect` request: pattern count, each pattern under the same path rule as
/// the virtual filesystem, and file/byte caps within bounds.
pub fn validate_collect(req: &CollectRequest, bounds: &RequestBounds) -> Result<(), SandboxError> {
    let patterns = req.patterns.len() as u64;
    if patterns > u64::from(bounds.max_collect_patterns) {
        return Err(too_large(
            BoundKind::CollectPatterns,
            patterns,
            u64::from(bounds.max_collect_patterns),
        ));
    }
    for pattern in &req.patterns {
        // Glob metacharacters are ordinary characters for the path rule.
        if let Err(ailu_fs_backend::FsError::InvalidPath { reason }) = normalize_path(pattern) {
            return Err(SandboxError::InvalidPath { reason });
        }
    }
    if req.max_files > bounds.max_collect_files {
        return Err(too_large(
            BoundKind::CollectFiles,
            u64::from(req.max_files),
            u64::from(bounds.max_collect_files),
        ));
    }
    if req.max_bytes > bounds.max_collect_bytes {
        return Err(too_large(
            BoundKind::CollectBytes,
            req.max_bytes,
            bounds.max_collect_bytes,
        ));
    }
    Ok(())
}

/// An exact host name: non-empty, no wildcard, no range or path, no whitespace or control
/// character, at most 253 bytes.
fn is_exact_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host
            .chars()
            .any(|c| c == '*' || c == '/' || c == '\\' || c.is_whitespace() || c.is_control())
}

/// POSIX-style environment variable name: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::LimitField;
    use crate::ids::TenantId;
    use crate::path::LogicalPath;
    use crate::request::{EnvGrant, HostRule, SecretRef};
    use ailu_graph_core::{NodeId, RunId};

    fn denial(result: Result<(), SandboxError>) -> PolicyReason {
        match result {
            Err(SandboxError::PolicyDenied(reason)) => reason,
            other => panic!("expected a policy denial, got {other:?}"),
        }
    }

    fn create() -> CreateRequest {
        CreateRequest {
            profile: "default".to_owned(),
            limits: Limits::default(),
            network: NetworkPolicy::Deny,
            inputs: vec![],
            env: vec![],
            ttl_ms: 60_000,
        }
    }

    fn exec(argv: &[&str]) -> ExecRequest {
        ExecRequest {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            cwd: LogicalPath::new(".work").unwrap(),
            stdin: None,
        }
    }

    fn collect(patterns: &[&str]) -> CollectRequest {
        CollectRequest {
            patterns: patterns.iter().map(|s| (*s).to_owned()).collect(),
            max_files: 10,
            max_bytes: 1024,
        }
    }

    #[test]
    fn context_requires_identity_and_one_based_attempt() {
        let ok = SandboxContext {
            tenant_id: TenantId::new("t"),
            run_id: RunId::from("r"),
            node_id: NodeId::from("n"),
            attempt: 1,
        };
        assert!(validate_context(&ok).is_ok());
        let zero = SandboxContext {
            attempt: 0,
            ..ok.clone()
        };
        assert_eq!(
            denial(validate_context(&zero)),
            PolicyReason::InvalidContext
        );
        let no_tenant = SandboxContext {
            tenant_id: TenantId::new(""),
            ..ok
        };
        assert_eq!(
            denial(validate_context(&no_tenant)),
            PolicyReason::InvalidContext
        );
    }

    #[test]
    fn a_default_create_request_is_accepted() {
        assert!(validate_create(&create(), &RequestBounds::default(), &Limits::default()).is_ok());
    }

    #[test]
    fn limit_above_ceiling_is_refused_not_clamped() {
        let mut req = create();
        req.limits.memory_bytes = Limits::default().memory_bytes + 1;
        let reason = denial(validate_create(
            &req,
            &RequestBounds::default(),
            &Limits::default(),
        ));
        assert_eq!(
            reason,
            PolicyReason::LimitAboveCeiling {
                field: LimitField::MemoryBytes
            }
        );
    }

    #[test]
    fn a_higher_ceiling_admits_a_higher_limit() {
        let mut req = create();
        req.limits.wall_time_ms = 60_000;
        let ceiling = Limits {
            wall_time_ms: 120_000,
            ..Limits::default()
        };
        assert!(validate_create(&req, &RequestBounds::default(), &ceiling).is_ok());
    }

    #[test]
    fn zero_limit_is_refused() {
        let mut req = create();
        req.limits.cpu_millicores = 0;
        assert_eq!(
            denial(validate_create(
                &req,
                &RequestBounds::default(),
                &Limits::default()
            )),
            PolicyReason::LimitZero {
                field: LimitField::CpuMillicores
            }
        );
    }

    #[test]
    fn ttl_above_the_bound_is_refused() {
        let bounds = RequestBounds::default();
        let mut req = create();
        req.ttl_ms = bounds.max_ttl_ms + 1;
        assert_eq!(
            denial(validate_create(&req, &bounds, &Limits::default())),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::Ttl,
                actual: bounds.max_ttl_ms + 1,
                max: bounds.max_ttl_ms
            }
        );
        req.ttl_ms = bounds.max_ttl_ms;
        assert!(validate_create(&req, &bounds, &Limits::default()).is_ok());
    }

    #[test]
    fn too_many_inputs_are_refused() {
        let bounds = RequestBounds {
            max_input_files: 1,
            ..RequestBounds::default()
        };
        let input = crate::request::InputRef {
            artifact: ailu_fs_backend::ArtifactRef {
                id: "a".into(),
                version: 1,
            },
            dest: LogicalPath::new("in/a.txt").unwrap(),
        };
        let mut req = create();
        req.inputs = vec![input.clone(), input];
        assert_eq!(
            denial(validate_create(&req, &bounds, &Limits::default())),
            PolicyReason::InputTooLarge {
                bound: BoundKind::InputFiles,
                actual: 2,
                max: 1
            }
        );
    }

    #[test]
    fn network_rules_must_be_exact_hosts() {
        for bad in ["*.example.com", "10.0.0.0/8", "", "a b", "h/path", "h\n"] {
            let mut req = create();
            req.network = NetworkPolicy::Allow(vec![HostRule {
                host: bad.to_owned(),
                port: 443,
            }]);
            assert!(
                matches!(
                    denial(validate_create(
                        &req,
                        &RequestBounds::default(),
                        &Limits::default()
                    )),
                    PolicyReason::NetworkRule { .. }
                ),
                "{bad:?} must be refused"
            );
        }
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![HostRule {
            host: "api.example.com".to_owned(),
            port: 0,
        }]);
        assert!(validate_create(&req, &RequestBounds::default(), &Limits::default()).is_err());
        req.network = NetworkPolicy::Allow(vec![HostRule {
            host: "api.example.com".to_owned(),
            port: 443,
        }]);
        assert!(validate_create(&req, &RequestBounds::default(), &Limits::default()).is_ok());
    }

    #[test]
    fn env_names_are_checked_and_unique() {
        let grant = |name: &str| EnvGrant {
            name: name.to_owned(),
            secret_ref: SecretRef("r".to_owned()),
        };
        for bad in ["", "1A", "A-B", "A=B", "A B"] {
            let mut req = create();
            req.env = vec![grant(bad)];
            assert!(
                matches!(
                    denial(validate_create(
                        &req,
                        &RequestBounds::default(),
                        &Limits::default()
                    )),
                    PolicyReason::InvalidEnvName { .. }
                ),
                "{bad:?} must be refused"
            );
        }
        let mut req = create();
        req.env = vec![grant("API_KEY"), grant("API_KEY")];
        assert!(validate_create(&req, &RequestBounds::default(), &Limits::default()).is_err());
        req.env = vec![grant("API_KEY"), grant("_OTHER1")];
        assert!(validate_create(&req, &RequestBounds::default(), &Limits::default()).is_ok());
    }

    #[test]
    fn echoed_names_are_clipped() {
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![HostRule {
            host: format!("{}*", "a".repeat(500)),
            port: 1,
        }]);
        match denial(validate_create(
            &req,
            &RequestBounds::default(),
            &Limits::default(),
        )) {
            PolicyReason::NetworkRule { host } => assert_eq!(host.len(), 64),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_argv_is_refused() {
        assert_eq!(
            denial(validate_exec(&exec(&[]), &RequestBounds::default())),
            PolicyReason::EmptyArgv
        );
    }

    #[test]
    fn empty_program_and_nul_are_refused() {
        let bounds = RequestBounds::default();
        assert_eq!(
            denial(validate_exec(&exec(&["", "x"]), &bounds)),
            PolicyReason::EmptyProgram
        );
        assert_eq!(
            denial(validate_exec(&exec(&["ls", "a\0b"]), &bounds)),
            PolicyReason::NulInArgument
        );
    }

    #[test]
    fn too_many_arguments_are_refused() {
        let bounds = RequestBounds {
            max_argv_count: 3,
            ..RequestBounds::default()
        };
        assert!(validate_exec(&exec(&["a", "b", "c"]), &bounds).is_ok());
        assert_eq!(
            denial(validate_exec(&exec(&["a", "b", "c", "d"]), &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::ArgvCount,
                actual: 4,
                max: 3
            }
        );
    }

    #[test]
    fn oversized_argv_bytes_are_refused() {
        let bounds = RequestBounds {
            max_argv_bytes: 8,
            ..RequestBounds::default()
        };
        assert!(validate_exec(&exec(&["abcd", "efgh"]), &bounds).is_ok());
        assert_eq!(
            denial(validate_exec(&exec(&["abcd", "efghi"]), &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::ArgvBytes,
                actual: 9,
                max: 8
            }
        );
    }

    #[test]
    fn oversized_stdin_is_refused() {
        let bounds = RequestBounds {
            max_stdin_bytes: 4,
            ..RequestBounds::default()
        };
        let mut req = exec(&["cat"]);
        req.stdin = Some(vec![0; 4]);
        assert!(validate_exec(&req, &bounds).is_ok());
        req.stdin = Some(vec![0; 5]);
        assert_eq!(
            denial(validate_exec(&req, &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::StdinBytes,
                actual: 5,
                max: 4
            }
        );
    }

    #[test]
    fn refusal_reasons_never_contain_argument_or_stdin_content() {
        let mut req = exec(&["tool", "--token=s3cr3t-value"]);
        req.stdin = Some(b"s3cr3t-stdin".to_vec());
        let bounds = RequestBounds {
            max_argv_count: 1,
            ..RequestBounds::default()
        };
        let err = validate_exec(&req, &bounds).unwrap_err();
        let rendered = format!("{err:?} {err}");
        assert!(!rendered.contains("s3cr3t"));
    }

    #[test]
    fn collect_patterns_follow_the_path_rule() {
        let bounds = RequestBounds::default();
        assert!(validate_collect(&collect(&["out/**/*.txt", "*.json"]), &bounds).is_ok());
        for bad in ["../x", "/abs/*", "a\\b", "a\0b"] {
            assert_eq!(
                validate_collect(&collect(&[bad]), &bounds)
                    .unwrap_err()
                    .code(),
                "invalid_path",
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn collect_bounds_are_enforced() {
        let bounds = RequestBounds {
            max_collect_patterns: 1,
            max_collect_files: 5,
            max_collect_bytes: 100,
            ..RequestBounds::default()
        };
        assert_eq!(
            denial(validate_collect(&collect(&["a", "b"]), &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::CollectPatterns,
                actual: 2,
                max: 1
            }
        );
        let mut req = collect(&["a"]);
        req.max_files = 6;
        assert_eq!(
            denial(validate_collect(&req, &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::CollectFiles,
                actual: 6,
                max: 5
            }
        );
        req.max_files = 5;
        req.max_bytes = 101;
        assert_eq!(
            denial(validate_collect(&req, &bounds)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::CollectBytes,
                actual: 101,
                max: 100
            }
        );
        req.max_bytes = 100;
        assert!(validate_collect(&req, &bounds).is_ok());
    }
}
