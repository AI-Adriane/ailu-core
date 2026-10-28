//! Pure validation of requests against deployment-set limits and bounds.
//!
//! Every function is fail-closed, has no side effect, and returns a typed
//! [`SandboxError::PolicyDenied`] (or [`SandboxError::InvalidPath`] for a bad path). Refusal
//! reasons carry positions, sizes and problem kinds, never the content that was refused
//! (no argument, stdin, host or variable name).

use crate::error::{BoundKind, NameProblem, PolicyReason, RuleProblem, SandboxError};
use crate::ids::SandboxContext;
use crate::limits::{Limits, RequestBounds};
use crate::request::{CollectRequest, CreateRequest, ExecRequest, HostRule, NetworkPolicy};
use ailu_fs_backend::normalize_path;
use std::collections::HashSet;

/// Longest isolation profile name, in bytes.
pub const MAX_PROFILE_BYTES: usize = 128;
/// Longest environment variable name, in bytes.
pub const MAX_ENV_NAME_BYTES: usize = 128;
/// Longest secret reference, in bytes.
pub const MAX_SECRET_REF_BYTES: usize = 256;
/// Most environment grants in one request.
pub const MAX_ENV_GRANTS: usize = 64;
/// Most network rules in one request.
pub const MAX_HOST_RULES: usize = 32;
/// Longest collect pattern, in bytes.
pub const MAX_COLLECT_PATTERN_BYTES: usize = 256;
const MAX_HOST_BYTES: usize = 253;
const MAX_LABEL_BYTES: usize = 63;

fn denied(reason: PolicyReason) -> SandboxError {
    SandboxError::PolicyDenied(reason)
}

fn too_large(bound: BoundKind, actual: u64, max: u64) -> SandboxError {
    denied(PolicyReason::RequestTooLarge { bound, actual, max })
}

fn index(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
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

/// Validate a `create` request: profile, limits (non-zero, not above `ceiling`), `ttl`,
/// inputs, network rules and environment grants.
///
/// [`BoundKind::InputBytes`] is **not** produced here: the total size of the inputs needs
/// the artifact store, so the guard checks it before calling the backend.
pub fn validate_create(
    req: &CreateRequest,
    bounds: &RequestBounds,
    ceiling: &Limits,
) -> Result<(), SandboxError> {
    if req.profile.is_empty() {
        return Err(denied(PolicyReason::EmptyField {
            bound: BoundKind::Profile,
        }));
    }
    if req.profile.len() > MAX_PROFILE_BYTES {
        return Err(too_large(
            BoundKind::Profile,
            req.profile.len() as u64,
            MAX_PROFILE_BYTES as u64,
        ));
    }
    if let Some(field) = req.limits.first_zero() {
        return Err(denied(PolicyReason::LimitZero { field }));
    }
    if let Some(field) = req.limits.first_above(ceiling) {
        return Err(denied(PolicyReason::LimitAboveCeiling { field }));
    }
    if req.ttl_ms == 0 {
        return Err(denied(PolicyReason::TtlZero));
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
        validate_rules(rules)?;
    }
    validate_env(req)
}

fn validate_rules(rules: &[HostRule]) -> Result<(), SandboxError> {
    if rules.is_empty() {
        return Err(denied(PolicyReason::NetworkAllowEmpty));
    }
    if rules.len() > MAX_HOST_RULES {
        return Err(too_large(
            BoundKind::HostRules,
            rules.len() as u64,
            MAX_HOST_RULES as u64,
        ));
    }
    for (i, rule) in rules.iter().enumerate() {
        let problem = if rule.port == 0 {
            Some(RuleProblem::BadPort)
        } else {
            host_problem(&rule.host)
        };
        if let Some(problem) = problem {
            return Err(denied(PolicyReason::NetworkRule {
                index: index(i),
                problem,
            }));
        }
    }
    Ok(())
}

fn validate_env(req: &CreateRequest) -> Result<(), SandboxError> {
    if req.env.len() > MAX_ENV_GRANTS {
        return Err(too_large(
            BoundKind::EnvGrants,
            req.env.len() as u64,
            MAX_ENV_GRANTS as u64,
        ));
    }
    let mut seen = HashSet::new();
    for (i, grant) in req.env.iter().enumerate() {
        let name_problem = |problem| {
            denied(PolicyReason::InvalidEnvName {
                index: index(i),
                problem,
            })
        };
        if grant.name.is_empty() {
            return Err(name_problem(NameProblem::Empty));
        }
        if grant.name.len() > MAX_ENV_NAME_BYTES {
            return Err(too_large(
                BoundKind::EnvName,
                grant.name.len() as u64,
                MAX_ENV_NAME_BYTES as u64,
            ));
        }
        if !is_env_name(&grant.name) {
            return Err(name_problem(NameProblem::BadCharacter));
        }
        if !seen.insert(grant.name.as_str()) {
            return Err(name_problem(NameProblem::Duplicate));
        }
        let secret_ref = &grant.secret_ref.0;
        if secret_ref.is_empty() {
            return Err(denied(PolicyReason::EmptyField {
                bound: BoundKind::SecretRef,
            }));
        }
        if secret_ref.len() > MAX_SECRET_REF_BYTES {
            return Err(too_large(
                BoundKind::SecretRef,
                secret_ref.len() as u64,
                MAX_SECRET_REF_BYTES as u64,
            ));
        }
    }
    Ok(())
}

/// Validate an `exec` request: non-empty `argv` within count and byte bounds, no null byte,
/// `stdin` within bound. The working directory is an optional [`crate::LogicalPath`],
/// already valid.
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
        if pattern.len() > MAX_COLLECT_PATTERN_BYTES {
            return Err(too_large(
                BoundKind::CollectPattern,
                pattern.len() as u64,
                MAX_COLLECT_PATTERN_BYTES as u64,
            ));
        }
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

/// Why `host` is not an acceptable exact DNS name, if it is not. Accepted: ASCII lowercase
/// labels `[a-z0-9-]` (no hyphen at a label edge) separated by single dots, 1 to 63 bytes
/// per label, 253 bytes at most, no trailing dot, not `localhost`, and no IP literal.
fn host_problem(host: &str) -> Option<RuleProblem> {
    if host.is_empty() {
        return Some(RuleProblem::Empty);
    }
    if host.len() > MAX_HOST_BYTES {
        return Some(RuleProblem::TooLong);
    }
    if host == "localhost" {
        return Some(RuleProblem::Localhost);
    }
    // IPv6 literals (bare or bracketed) always contain `:` or `[`.
    if host.contains(':') || host.starts_with('[') {
        return Some(RuleProblem::IpLiteral);
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    {
        return Some(RuleProblem::BadCharacter);
    }
    let labels: Vec<&str> = host.split('.').collect();
    for label in &labels {
        if label.is_empty() {
            return Some(RuleProblem::EmptyLabel);
        }
        if label.len() > MAX_LABEL_BYTES {
            return Some(RuleProblem::LabelTooLong);
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Some(RuleProblem::BadCharacter);
        }
    }
    // A numeric last label means an IPv4 literal (or a shorthand such as `127.1`).
    if labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
    {
        return Some(RuleProblem::IpLiteral);
    }
    None
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
    use crate::error::{LimitField, NameProblem, RuleProblem};
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

    fn with_rule(host: &str, port: u16) -> CreateRequest {
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![HostRule {
            host: host.to_owned(),
            port,
        }]);
        req
    }

    fn check(req: &CreateRequest) -> Result<(), SandboxError> {
        validate_create(req, &RequestBounds::default(), &Limits::default())
    }

    #[test]
    fn zero_ttl_is_refused() {
        let mut req = create();
        req.ttl_ms = 0;
        assert_eq!(denial(check(&req)), PolicyReason::TtlZero);
        req.ttl_ms = 1;
        assert!(check(&req).is_ok());
    }

    #[test]
    fn pathological_hosts_are_refused() {
        let cases: &[(&str, RuleProblem)] = &[
            ("127.0.0.1", RuleProblem::IpLiteral),
            ("169.254.169.254", RuleProblem::IpLiteral),
            ("127.1", RuleProblem::IpLiteral),
            ("[::1]", RuleProblem::IpLiteral),
            ("::1", RuleProblem::IpLiteral),
            ("a:b", RuleProblem::IpLiteral),
            ("localhost", RuleProblem::Localhost),
            ("user@h", RuleProblem::BadCharacter),
            ("h?x", RuleProblem::BadCharacter),
            ("h/path", RuleProblem::BadCharacter),
            ("*.example.com", RuleProblem::BadCharacter),
            ("EXAMPLE.com.", RuleProblem::BadCharacter),
            ("example.com.", RuleProblem::EmptyLabel),
            ("\u{e9}.com", RuleProblem::BadCharacter),
            ("a..b", RuleProblem::EmptyLabel),
            (".a", RuleProblem::EmptyLabel),
            ("-a.com", RuleProblem::BadCharacter),
            ("a b", RuleProblem::BadCharacter),
            ("h\n", RuleProblem::BadCharacter),
            ("", RuleProblem::Empty),
        ];
        for (host, expected) in cases {
            assert_eq!(
                denial(check(&with_rule(host, 443))),
                PolicyReason::NetworkRule {
                    index: 0,
                    problem: *expected
                },
                "{host:?}"
            );
        }
    }

    #[test]
    fn host_length_limits_are_enforced() {
        let label63 = "a".repeat(63);
        assert!(check(&with_rule(&label63, 443)).is_ok());
        assert_eq!(
            denial(check(&with_rule(&"a".repeat(64), 443))),
            PolicyReason::NetworkRule {
                index: 0,
                problem: RuleProblem::LabelTooLong
            }
        );
        // 4 labels of 62 + 3 dots = 251 bytes: accepted. One more byte per label overshoots.
        let ok = vec!["a".repeat(62); 4].join(".");
        assert!(check(&with_rule(&ok, 443)).is_ok());
        let too_long = vec!["a".repeat(63); 4].join(".");
        assert_eq!(too_long.len(), 255);
        assert_eq!(
            denial(check(&with_rule(&too_long, 443))),
            PolicyReason::NetworkRule {
                index: 0,
                problem: RuleProblem::TooLong
            }
        );
    }

    #[test]
    fn exact_lowercase_dns_names_are_accepted() {
        for host in ["api.example.com", "a-b.example.org", "x1.y2.z3.io", "a"] {
            assert!(check(&with_rule(host, 443)).is_ok(), "{host:?}");
        }
        // A numeric label that is not the last one is not an IP literal.
        assert!(check(&with_rule("1.example.com", 1)).is_ok());
    }

    #[test]
    fn ports_are_one_to_65535() {
        assert_eq!(
            denial(check(&with_rule("api.example.com", 0))),
            PolicyReason::NetworkRule {
                index: 0,
                problem: RuleProblem::BadPort
            }
        );
        assert!(check(&with_rule("api.example.com", 1)).is_ok());
        assert!(check(&with_rule("api.example.com", 65535)).is_ok());
    }

    #[test]
    fn allow_with_no_rule_is_refused() {
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![]);
        assert_eq!(denial(check(&req)), PolicyReason::NetworkAllowEmpty);
    }

    #[test]
    fn too_many_rules_are_refused() {
        let rules = |n: usize| {
            NetworkPolicy::Allow(
                (0..n)
                    .map(|i| HostRule {
                        host: format!("h{i}.example.com"),
                        port: 443,
                    })
                    .collect(),
            )
        };
        let mut req = create();
        req.network = rules(MAX_HOST_RULES);
        assert!(check(&req).is_ok());
        req.network = rules(MAX_HOST_RULES + 1);
        assert_eq!(
            denial(check(&req)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::HostRules,
                actual: 33,
                max: 32
            }
        );
    }

    #[test]
    fn rule_index_points_at_the_offending_rule() {
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![
            HostRule {
                host: "ok.example.com".to_owned(),
                port: 443,
            },
            HostRule {
                host: "bad host".to_owned(),
                port: 443,
            },
        ]);
        assert_eq!(
            denial(check(&req)),
            PolicyReason::NetworkRule {
                index: 1,
                problem: RuleProblem::BadCharacter
            }
        );
    }

    fn grant(name: &str) -> EnvGrant {
        EnvGrant {
            name: name.to_owned(),
            secret_ref: SecretRef("r".to_owned()),
        }
    }

    #[test]
    fn env_names_are_checked_and_unique() {
        for bad in ["1A", "A-B", "A=B", "A B", "\u{e9}"] {
            let mut req = create();
            req.env = vec![grant(bad)];
            assert_eq!(
                denial(check(&req)),
                PolicyReason::InvalidEnvName {
                    index: 0,
                    problem: NameProblem::BadCharacter
                },
                "{bad:?}"
            );
        }
        let mut req = create();
        req.env = vec![grant("")];
        assert_eq!(
            denial(check(&req)),
            PolicyReason::InvalidEnvName {
                index: 0,
                problem: NameProblem::Empty
            }
        );
        req.env = vec![grant("API_KEY"), grant("API_KEY")];
        assert_eq!(
            denial(check(&req)),
            PolicyReason::InvalidEnvName {
                index: 1,
                problem: NameProblem::Duplicate
            }
        );
        req.env = vec![grant("API_KEY"), grant("_OTHER1")];
        assert!(check(&req).is_ok());
    }

    #[test]
    fn env_and_secret_bounds_are_enforced() {
        let mut req = create();
        req.env = vec![grant(&"A".repeat(MAX_ENV_NAME_BYTES))];
        assert!(check(&req).is_ok());
        req.env = vec![grant(&"A".repeat(MAX_ENV_NAME_BYTES + 1))];
        assert_eq!(
            denial(check(&req)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::EnvName,
                actual: 129,
                max: 128
            }
        );
        let mut long_ref = grant("A");
        long_ref.secret_ref = SecretRef("x".repeat(MAX_SECRET_REF_BYTES + 1));
        req.env = vec![long_ref];
        assert_eq!(
            denial(check(&req)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::SecretRef,
                actual: 257,
                max: 256
            }
        );
        let mut empty_ref = grant("A");
        empty_ref.secret_ref = SecretRef(String::new());
        req.env = vec![empty_ref];
        assert_eq!(
            denial(check(&req)),
            PolicyReason::EmptyField {
                bound: BoundKind::SecretRef
            }
        );
        req.env = (0..=MAX_ENV_GRANTS)
            .map(|i| grant(&format!("V{i}")))
            .collect();
        assert_eq!(
            denial(check(&req)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::EnvGrants,
                actual: 65,
                max: 64
            }
        );
    }

    #[test]
    fn profile_must_be_non_empty_and_bounded() {
        let mut req = create();
        req.profile = String::new();
        assert_eq!(
            denial(check(&req)),
            PolicyReason::EmptyField {
                bound: BoundKind::Profile
            }
        );
        req.profile = "p".repeat(MAX_PROFILE_BYTES);
        assert!(check(&req).is_ok());
        req.profile = "p".repeat(MAX_PROFILE_BYTES + 1);
        assert_eq!(
            denial(check(&req)),
            PolicyReason::RequestTooLarge {
                bound: BoundKind::Profile,
                actual: 129,
                max: 128
            }
        );
    }

    #[test]
    fn refusals_never_echo_the_refused_content() {
        let mut req = create();
        req.network = NetworkPolicy::Allow(vec![HostRule {
            host: "user:pass@host".to_owned(),
            port: 443,
        }]);
        let rendered = format!("{:?}", check(&req).unwrap_err());
        assert!(
            !rendered.contains("pass") && !rendered.contains("user"),
            "{rendered}"
        );
        req.network = NetworkPolicy::Deny;
        req.env = vec![grant("API_KEY=hunter2")];
        let err = check(&req).unwrap_err();
        let rendered = format!("{err:?} {err} {}", serde_json::to_string(&err).unwrap());
        assert!(
            !rendered.contains("hunter2") && !rendered.contains("API_KEY"),
            "{rendered}"
        );
        // Oversized names report their length only.
        req.env = vec![grant(&format!("{}hunter2", "A".repeat(200)))];
        let err = check(&req).unwrap_err();
        assert!(!format!("{err:?}").contains("hunter2"));
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
