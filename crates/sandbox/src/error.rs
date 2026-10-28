//! Typed sandbox errors with stable codes.

use crate::outcome::ExecOutcome;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle state of a sandbox. `Poisoned` accepts only `destroy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Created,
    Executing,
    Executed,
    Collected,
    Destroyed,
    Poisoned,
}

/// Admission control dimension that was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaKind {
    /// Too many simultaneous sandboxes for the tenant.
    TenantConcurrency,
    /// Sandbox creation rate for the tenant is too high.
    CreationRate,
    /// The guard's own registry is full (all tenants together).
    RegistryFull,
    /// The tenant holds too many live sandbox records.
    TenantRegistryFull,
    /// The tenant (or the table as a whole) cannot take another isolation domain.
    IsolationDomainsFull,
}

/// Limit that was exceeded while a program was running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    WallTime,
    Memory,
    Processes,
    Disk,
}

/// A field of [`crate::Limits`], used to name which requested limit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitField {
    CpuMillicores,
    MemoryBytes,
    WallTimeMs,
    OutputBytes,
    MaxProcesses,
    EphemeralDiskBytes,
}

/// A bound of [`crate::RequestBounds`], used to name which bound a request went over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundKind {
    ArgvCount,
    ArgvBytes,
    StdinBytes,
    CollectPatterns,
    CollectFiles,
    CollectBytes,
    Ttl,
    InputFiles,
    /// Reserved: the total size of the inputs is checked by the guard, which can see the
    /// artifact store; pure validation never produces this bound.
    InputBytes,
    Profile,
    EnvName,
    SecretRef,
    EnvGrants,
    HostRules,
    CollectPattern,
}

/// What is wrong with a network rule. Positions are indexes in the request, never content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleProblem {
    Empty,
    TooLong,
    /// Empty label (leading, trailing or doubled dot).
    EmptyLabel,
    LabelTooLong,
    /// A character outside `[a-z0-9-]` (or a hyphen at the edge of a label).
    BadCharacter,
    /// An IPv4 or IPv6 literal, bracketed or not. Only DNS names are accepted.
    IpLiteral,
    Localhost,
    BadPort,
    /// A well-formed rule that the deployment policy does not allow.
    NotInPolicy,
}

/// What is wrong with an environment variable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameProblem {
    Empty,
    BadCharacter,
    Duplicate,
}

/// Why a request was refused by policy. Carries identifiers and numbers, never content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyReason {
    /// The context is unusable (empty identity segment, attempt of zero).
    InvalidContext,
    /// `argv` is empty.
    EmptyArgv,
    /// `argv[0]` (the program) is empty.
    EmptyProgram,
    /// An argument contains a null byte.
    NulInArgument,
    /// A request-size bound was exceeded.
    RequestTooLarge {
        bound: BoundKind,
        actual: u64,
        max: u64,
    },
    /// The declared inputs exceed the input bounds.
    InputTooLarge {
        bound: BoundKind,
        actual: u64,
        max: u64,
    },
    /// A requested limit is above the deployment ceiling.
    LimitAboveCeiling { field: LimitField },
    /// A requested limit is zero, which no program can run under.
    LimitZero { field: LimitField },
    /// The input at `index` has a destination equal to, inside, or containing another one.
    InputPathConflict { index: u32 },
    /// `ttl_ms` is zero.
    TtlZero,
    /// A mandatory field is empty (`bound` names it).
    EmptyField { bound: BoundKind },
    /// `Allow` with no rule: refused so that "allow nothing" is spelled `Deny`.
    NetworkAllowEmpty,
    /// The network rule at `index` is malformed (or outside the deployment policy).
    NetworkRule { index: u32, problem: RuleProblem },
    /// The program is not allowed by the deployment policy.
    ProgramNotAllowed,
    /// A secret grant is malformed or not covered by the approval.
    SecretNotCovered { index: u32 },
    /// The environment variable name at `index` is malformed or granted twice.
    InvalidEnvName { index: u32, problem: NameProblem },
    /// The attempt started before the result retention window: the guard cannot tell a call
    /// that never ran from one whose stored result expired, so it refuses (fail closed).
    AttemptTooOld,
    /// The run is said to have started in the future, beyond the allowed clock skew.
    AttemptInFuture,
    /// The approval was given by the principal that requested the call.
    SelfApproval,
    /// The call cannot be shown to an approver in full (an argument or a field is too long,
    /// or there are too many arguments), so it is refused rather than approved unseen.
    UnreviewableCall,
    /// A control, escape, bidirectional-control, line-separator or invisible format
    /// character in text the approver must read.
    UnsafeText,
}

impl PolicyReason {
    fn code(&self) -> &'static str {
        match self {
            PolicyReason::InvalidContext => "policy_denied.invalid_context",
            PolicyReason::EmptyArgv => "policy_denied.empty_argv",
            PolicyReason::EmptyProgram => "policy_denied.empty_program",
            PolicyReason::NulInArgument => "policy_denied.nul_in_argument",
            PolicyReason::RequestTooLarge { .. } => "policy_denied.request_too_large",
            PolicyReason::InputTooLarge { .. } => "policy_denied.input_too_large",
            PolicyReason::LimitAboveCeiling { .. } => "policy_denied.limit_above_ceiling",
            PolicyReason::LimitZero { .. } => "policy_denied.limit_zero",
            PolicyReason::InputPathConflict { .. } => "policy_denied.input_path_conflict",
            PolicyReason::TtlZero => "policy_denied.ttl_zero",
            PolicyReason::EmptyField { .. } => "policy_denied.empty_field",
            PolicyReason::NetworkAllowEmpty => "policy_denied.network_allow_empty",
            PolicyReason::NetworkRule { .. } => "policy_denied.network_rule",
            PolicyReason::ProgramNotAllowed => "policy_denied.program_not_allowed",
            PolicyReason::SecretNotCovered { .. } => "policy_denied.secret_not_covered",
            PolicyReason::InvalidEnvName { .. } => "policy_denied.invalid_env_name",
            PolicyReason::AttemptTooOld => "policy_denied.attempt_too_old",
            PolicyReason::AttemptInFuture => "policy_denied.attempt_in_future",
            PolicyReason::SelfApproval => "policy_denied.self_approval",
            PolicyReason::UnreviewableCall => "policy_denied.unreviewable_call",
            PolicyReason::UnsafeText => "policy_denied.unsafe_text",
        }
    }
}

impl QuotaKind {
    fn code(self) -> &'static str {
        match self {
            QuotaKind::TenantConcurrency => "quota_exceeded.tenant_concurrency",
            QuotaKind::CreationRate => "quota_exceeded.creation_rate",
            QuotaKind::RegistryFull => "quota_exceeded.registry_full",
            QuotaKind::TenantRegistryFull => "quota_exceeded.tenant_registry_full",
            QuotaKind::IsolationDomainsFull => "quota_exceeded.isolation_domains_full",
        }
    }
}

impl LimitKind {
    fn code(self) -> &'static str {
        match self {
            LimitKind::WallTime => "limit_exceeded.wall_time",
            LimitKind::Memory => "limit_exceeded.memory",
            LimitKind::Processes => "limit_exceeded.processes",
            LimitKind::Disk => "limit_exceeded.disk",
        }
    }
}

/// Approval subject carried by [`SandboxError::ApprovalRequired`]. It serializes as the plain
/// JSON value, but its `Debug` output is masked: the subject describes the call being
/// approved and must not end up in logs through `{:?}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalSubject(pub Value);

impl std::fmt::Debug for ApprovalSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApprovalSubject(..)")
    }
}

/// Every way a sandbox operation can fail.
///
/// Callers distinguish a definitive **error**, a **wait** ([`SandboxError::Busy`]) and an
/// **unavailability** ([`SandboxError::Unavailable`]); only the last two are worth retrying.
/// Never parse a message: branch on [`SandboxError::code`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum SandboxError {
    /// Refused by policy: limit above ceiling, network rule, request too large, …
    #[error("denied by policy: {}", .0.code())]
    PolicyDenied(PolicyReason),
    /// No valid approval for this exact call; the caller files the request and suspends.
    #[error("approval required")]
    ApprovalRequired { subject: ApprovalSubject },
    /// Admission control: refused immediately, never silently queued.
    #[error("quota exceeded: {}", kind.code())]
    QuotaExceeded { kind: QuotaKind },
    /// The backend killed the program because a limit was exceeded.
    #[error("limit exceeded: {}", kind.code())]
    LimitExceeded {
        kind: LimitKind,
        /// What the program produced before it was killed.
        partial: Box<ExecOutcome>,
    },
    /// The backend detected an isolation violation; the sandbox is poisoned.
    #[error("isolation violation")]
    IsolationViolation { signal: String },
    /// Same idempotency key, different request.
    #[error("idempotency conflict")]
    IdempotencyConflict,
    /// A path failed normalization.
    #[error("invalid path: {reason}")]
    InvalidPath { reason: String },
    /// Unknown sandbox, artifact or key — also the answer for another tenant's.
    #[error("not found")]
    NotFound,
    /// The operation is not allowed in the sandbox's current state.
    #[error("invalid state: {from:?}")]
    InvalidState { from: SandboxState },
    /// Cooperative cancellation was observed.
    #[error("cancelled")]
    Cancelled,
    /// Temporarily busy; retry later.
    #[error("busy")]
    Busy { retry_after_ms: Option<u64> },
    /// The backend is unavailable.
    #[error("unavailable (retriable: {retriable})")]
    Unavailable { retriable: bool },
    /// Last resort; the message must already be redacted.
    #[error("backend error: {0}")]
    Backend(String),
}

impl SandboxError {
    /// Stable top-level error code. Adding a code is compatible; changing or removing one
    /// is a breaking change.
    pub fn code(&self) -> &'static str {
        match self {
            SandboxError::PolicyDenied(_) => "policy_denied",
            SandboxError::ApprovalRequired { .. } => "approval_required",
            SandboxError::QuotaExceeded { .. } => "quota_exceeded",
            SandboxError::LimitExceeded { .. } => "limit_exceeded",
            SandboxError::IsolationViolation { .. } => "isolation_violation",
            SandboxError::IdempotencyConflict => "idempotency_conflict",
            SandboxError::InvalidPath { .. } => "invalid_path",
            SandboxError::NotFound => "not_found",
            SandboxError::InvalidState { .. } => "invalid_state",
            SandboxError::Cancelled => "cancelled",
            SandboxError::Busy { .. } => "busy",
            SandboxError::Unavailable { .. } => "unavailable",
            SandboxError::Backend(_) => "backend_error",
        }
    }

    /// Full code including the sub-code when the error has a sub-type
    /// (`limit_exceeded.wall_time`), otherwise the same as [`SandboxError::code`].
    pub fn full_code(&self) -> &'static str {
        match self {
            SandboxError::PolicyDenied(reason) => reason.code(),
            SandboxError::QuotaExceeded { kind } => kind.code(),
            SandboxError::LimitExceeded { kind, .. } => kind.code(),
            other => other.code(),
        }
    }

    /// `true` for the errors worth retrying: [`SandboxError::Busy`] and a retriable
    /// [`SandboxError::Unavailable`].
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            SandboxError::Busy { .. } | SandboxError::Unavailable { retriable: true }
        )
    }
}

impl From<crate::path::InvalidPath> for SandboxError {
    fn from(value: crate::path::InvalidPath) -> Self {
        SandboxError::InvalidPath {
            reason: value.reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::DeclaredExit;

    fn partial() -> Box<ExecOutcome> {
        Box::new(ExecOutcome {
            declared_exit: DeclaredExit::Killed {
                cause: LimitKind::WallTime,
            },
            truncated: false,
            truncation: vec![],
            declared_duration_ms: 1,
        })
    }

    /// Golden list: one value per variant and sub-type. This is the public wire contract;
    /// changing a string here is a breaking change.
    #[test]
    fn codes_are_stable() {
        let golden: Vec<(SandboxError, &str, &str)> = vec![
            (
                SandboxError::PolicyDenied(PolicyReason::EmptyArgv),
                "policy_denied",
                "policy_denied.empty_argv",
            ),
            (
                SandboxError::ApprovalRequired {
                    subject: ApprovalSubject(Value::Null),
                },
                "approval_required",
                "approval_required",
            ),
            (
                SandboxError::QuotaExceeded {
                    kind: QuotaKind::TenantConcurrency,
                },
                "quota_exceeded",
                "quota_exceeded.tenant_concurrency",
            ),
            (
                SandboxError::QuotaExceeded {
                    kind: QuotaKind::CreationRate,
                },
                "quota_exceeded",
                "quota_exceeded.creation_rate",
            ),
            (
                SandboxError::QuotaExceeded {
                    kind: QuotaKind::RegistryFull,
                },
                "quota_exceeded",
                "quota_exceeded.registry_full",
            ),
            (
                SandboxError::QuotaExceeded {
                    kind: QuotaKind::TenantRegistryFull,
                },
                "quota_exceeded",
                "quota_exceeded.tenant_registry_full",
            ),
            (
                SandboxError::QuotaExceeded {
                    kind: QuotaKind::IsolationDomainsFull,
                },
                "quota_exceeded",
                "quota_exceeded.isolation_domains_full",
            ),
            (
                SandboxError::LimitExceeded {
                    kind: LimitKind::WallTime,
                    partial: partial(),
                },
                "limit_exceeded",
                "limit_exceeded.wall_time",
            ),
            (
                SandboxError::LimitExceeded {
                    kind: LimitKind::Memory,
                    partial: partial(),
                },
                "limit_exceeded",
                "limit_exceeded.memory",
            ),
            (
                SandboxError::LimitExceeded {
                    kind: LimitKind::Processes,
                    partial: partial(),
                },
                "limit_exceeded",
                "limit_exceeded.processes",
            ),
            (
                SandboxError::LimitExceeded {
                    kind: LimitKind::Disk,
                    partial: partial(),
                },
                "limit_exceeded",
                "limit_exceeded.disk",
            ),
            (
                SandboxError::IsolationViolation {
                    signal: "x".to_owned(),
                },
                "isolation_violation",
                "isolation_violation",
            ),
            (
                SandboxError::IdempotencyConflict,
                "idempotency_conflict",
                "idempotency_conflict",
            ),
            (
                SandboxError::InvalidPath {
                    reason: "x".to_owned(),
                },
                "invalid_path",
                "invalid_path",
            ),
            (SandboxError::NotFound, "not_found", "not_found"),
            (
                SandboxError::InvalidState {
                    from: SandboxState::Created,
                },
                "invalid_state",
                "invalid_state",
            ),
            (SandboxError::Cancelled, "cancelled", "cancelled"),
            (
                SandboxError::Busy {
                    retry_after_ms: None,
                },
                "busy",
                "busy",
            ),
            (
                SandboxError::Unavailable { retriable: false },
                "unavailable",
                "unavailable",
            ),
            (
                SandboxError::Backend("x".to_owned()),
                "backend_error",
                "backend_error",
            ),
        ];
        for (error, code, full) in &golden {
            assert_eq!(error.code(), *code, "code of {error:?}");
            assert_eq!(error.full_code(), *full, "full code of {error:?}");
        }
        // The top-level set is exactly the thirteen contract codes.
        let mut top: Vec<&str> = golden.iter().map(|(e, _, _)| e.code()).collect();
        top.sort_unstable();
        top.dedup();
        assert_eq!(
            top,
            vec![
                "approval_required",
                "backend_error",
                "busy",
                "cancelled",
                "idempotency_conflict",
                "invalid_path",
                "invalid_state",
                "isolation_violation",
                "limit_exceeded",
                "not_found",
                "policy_denied",
                "quota_exceeded",
                "unavailable",
            ]
        );
    }

    #[test]
    fn policy_reason_sub_codes_are_stable() {
        let golden = [
            (
                PolicyReason::InvalidContext,
                "policy_denied.invalid_context",
            ),
            (PolicyReason::EmptyArgv, "policy_denied.empty_argv"),
            (PolicyReason::EmptyProgram, "policy_denied.empty_program"),
            (PolicyReason::NulInArgument, "policy_denied.nul_in_argument"),
            (
                PolicyReason::RequestTooLarge {
                    bound: BoundKind::ArgvCount,
                    actual: 2,
                    max: 1,
                },
                "policy_denied.request_too_large",
            ),
            (
                PolicyReason::InputTooLarge {
                    bound: BoundKind::InputFiles,
                    actual: 2,
                    max: 1,
                },
                "policy_denied.input_too_large",
            ),
            (
                PolicyReason::LimitAboveCeiling {
                    field: LimitField::MemoryBytes,
                },
                "policy_denied.limit_above_ceiling",
            ),
            (
                PolicyReason::LimitZero {
                    field: LimitField::WallTimeMs,
                },
                "policy_denied.limit_zero",
            ),
            (
                PolicyReason::NetworkRule {
                    index: 0,
                    problem: RuleProblem::Empty,
                },
                "policy_denied.network_rule",
            ),
            (
                PolicyReason::ProgramNotAllowed,
                "policy_denied.program_not_allowed",
            ),
            (
                PolicyReason::SecretNotCovered { index: 0 },
                "policy_denied.secret_not_covered",
            ),
            (
                PolicyReason::InvalidEnvName {
                    index: 0,
                    problem: NameProblem::Empty,
                },
                "policy_denied.invalid_env_name",
            ),
            (
                PolicyReason::InputPathConflict { index: 1 },
                "policy_denied.input_path_conflict",
            ),
            (PolicyReason::TtlZero, "policy_denied.ttl_zero"),
            (
                PolicyReason::EmptyField {
                    bound: BoundKind::Profile,
                },
                "policy_denied.empty_field",
            ),
            (
                PolicyReason::NetworkAllowEmpty,
                "policy_denied.network_allow_empty",
            ),
            (PolicyReason::AttemptTooOld, "policy_denied.attempt_too_old"),
            (
                PolicyReason::AttemptInFuture,
                "policy_denied.attempt_in_future",
            ),
            (PolicyReason::SelfApproval, "policy_denied.self_approval"),
            (
                PolicyReason::UnreviewableCall,
                "policy_denied.unreviewable_call",
            ),
            (PolicyReason::UnsafeText, "policy_denied.unsafe_text"),
        ];
        for (reason, code) in golden {
            assert_eq!(SandboxError::PolicyDenied(reason).full_code(), code);
        }
    }

    #[test]
    fn only_busy_and_retriable_unavailable_are_retriable() {
        assert!(SandboxError::Busy {
            retry_after_ms: Some(10)
        }
        .is_retriable());
        assert!(SandboxError::Unavailable { retriable: true }.is_retriable());
        assert!(!SandboxError::Unavailable { retriable: false }.is_retriable());
        assert!(!SandboxError::NotFound.is_retriable());
        assert!(!SandboxError::Cancelled.is_retriable());
    }

    #[test]
    fn display_never_contains_partial_output_or_subject() {
        let err = SandboxError::ApprovalRequired {
            subject: ApprovalSubject(serde_json::json!({"argv": ["secret-token"]})),
        };
        assert!(!err.to_string().contains("secret-token"));
        assert!(!format!("{err:?}").contains("secret-token"));
        // The wire form still carries the subject so the caller can file the request.
        let wire = serde_json::to_value(&err).unwrap();
        assert_eq!(wire["detail"]["subject"]["argv"][0], "secret-token");
    }

    #[test]
    fn serializes_with_a_type_tag() {
        let json = serde_json::to_value(SandboxError::NotFound).unwrap();
        assert_eq!(json["type"], "not_found");
        let back: SandboxError = serde_json::from_value(json).unwrap();
        assert_eq!(back, SandboxError::NotFound);
    }

    #[test]
    fn wire_tags_are_snake_case_like_the_codes() {
        let all = [
            (SandboxError::IdempotencyConflict, "idempotency_conflict"),
            (SandboxError::Backend("x".into()), "backend"),
            (
                SandboxError::PolicyDenied(PolicyReason::EmptyArgv),
                "policy_denied",
            ),
        ];
        for (error, tag) in all {
            assert_eq!(serde_json::to_value(&error).unwrap()["type"], tag);
        }
        let reason = serde_json::to_value(PolicyReason::NulInArgument).unwrap();
        assert_eq!(reason["kind"], "nul_in_argument");
    }

    #[test]
    fn invalid_path_converts_to_the_typed_error() {
        let err: SandboxError = crate::path::InvalidPath {
            reason: "absolute path".to_owned(),
        }
        .into();
        assert_eq!(err.code(), "invalid_path");
    }
}
