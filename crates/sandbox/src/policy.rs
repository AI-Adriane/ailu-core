//! Deployment policy: ceilings, allowed network destinations, per-tenant admission and
//! result retention. It comes from the deployment, never from an agent.

use crate::error::{PolicyReason, RuleProblem, SandboxError};
use crate::limits::{Limits, RequestBounds};
use crate::request::{CreateRequest, HostRule, NetworkPolicy};
use crate::validate::validate_create;

/// Default result retention: 24 hours.
pub const DEFAULT_RESULT_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// Highest retention a deployment may configure: 7 days.
pub const MAX_RESULT_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Admission ceilings applied to each tenant separately, so that one tenant reaching them
/// never slows another one down. Beyond them the guard answers `QuotaExceeded` at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantQuota {
    /// Sandboxes a tenant may hold at the same time (created and not yet destroyed).
    pub max_concurrent: u32,
    /// Sandboxes a tenant may create within any `window_ms` window.
    pub max_creations: u32,
    /// Length of the creation-rate window. A window of zero counts nothing, so the rate
    /// ceiling is then only reached when `max_creations` is zero.
    pub window_ms: u64,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_creations: 30,
            window_ms: 60_000,
        }
    }
}

/// A retention outside `(0, 7 days]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyConfigError {
    #[error("result retention must not be zero")]
    RetentionZero,
    #[error("result retention is above the 7 day ceiling")]
    RetentionAboveCeiling,
}

/// Everything the deployment decides about sandboxes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    /// Highest limits a request may ask for. A request above it is refused, never clamped.
    pub ceiling: Limits,
    /// Bounds on the request itself.
    pub bounds: RequestBounds,
    /// Network destinations a request may ask for. A requested rule must appear here
    /// exactly; the default is empty, so every `Allow` request is refused.
    pub network_allow: Vec<HostRule>,
    pub tenant_quota: TenantQuota,
    result_retention_ms: u64,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            ceiling: Limits::default(),
            bounds: RequestBounds::default(),
            network_allow: Vec::new(),
            tenant_quota: TenantQuota::default(),
            result_retention_ms: DEFAULT_RESULT_RETENTION_MS,
        }
    }
}

impl SandboxPolicy {
    /// How long a result stays reproducible. An attempt that started earlier is refused.
    pub fn result_retention_ms(&self) -> u64 {
        self.result_retention_ms
    }

    /// Set the result retention, from 1 ms up to [`MAX_RESULT_RETENTION_MS`].
    pub fn with_result_retention_ms(mut self, ms: u64) -> Result<Self, PolicyConfigError> {
        if ms == 0 {
            return Err(PolicyConfigError::RetentionZero);
        }
        if ms > MAX_RESULT_RETENTION_MS {
            return Err(PolicyConfigError::RetentionAboveCeiling);
        }
        self.result_retention_ms = ms;
        Ok(self)
    }

    /// Every requested rule must be one the deployment allows. The refusal names the index
    /// of the first rule outside the policy, never its content. `Deny` always passes.
    pub fn check_network(&self, network: &NetworkPolicy) -> Result<(), SandboxError> {
        if let NetworkPolicy::Allow(rules) = network {
            if let Some(i) = rules.iter().position(|r| !self.network_allow.contains(r)) {
                return Err(SandboxError::PolicyDenied(PolicyReason::NetworkRule {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    problem: RuleProblem::NotInPolicy,
                }));
            }
        }
        Ok(())
    }

    /// Validate a `create` request against this policy: the pure validation of the request
    /// (bounds, ceiling, shape) and then the deployment's network list.
    pub fn check_create(&self, req: &CreateRequest) -> Result<(), SandboxError> {
        validate_create(req, &self.bounds, &self.ceiling)?;
        self.check_network(&req.network)
    }

    /// Refuse an attempt whose run started more than the retention ago. The instant is a
    /// datum of the host; a start in the future (clock skew) is not refused.
    pub fn check_attempt_age(
        &self,
        now_ms: u64,
        run_started_at_ms: u64,
    ) -> Result<(), SandboxError> {
        if now_ms.saturating_sub(run_started_at_ms) > self.result_retention_ms {
            return Err(SandboxError::PolicyDenied(PolicyReason::AttemptTooOld));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str, port: u16) -> HostRule {
        HostRule {
            host: host.to_owned(),
            port,
        }
    }

    fn create(network: NetworkPolicy) -> CreateRequest {
        CreateRequest {
            profile: "default".to_owned(),
            limits: Limits::default(),
            network,
            inputs: vec![],
            env: vec![],
            ttl_ms: 1000,
        }
    }

    #[test]
    fn defaults_follow_the_adr() {
        let p = SandboxPolicy::default();
        assert_eq!(p.result_retention_ms(), 24 * 3600 * 1000);
        assert_eq!(MAX_RESULT_RETENTION_MS, 7 * 24 * 3600 * 1000);
        assert!(p.network_allow.is_empty());
        assert_eq!(p.ceiling, Limits::default());
        assert_eq!(p.bounds, RequestBounds::default());
    }

    #[test]
    fn retention_is_bounded_by_the_ceiling() {
        let p = SandboxPolicy::default();
        assert_eq!(
            p.clone().with_result_retention_ms(0),
            Err(PolicyConfigError::RetentionZero)
        );
        assert_eq!(
            p.clone()
                .with_result_retention_ms(MAX_RESULT_RETENTION_MS + 1),
            Err(PolicyConfigError::RetentionAboveCeiling)
        );
        let ok = p.with_result_retention_ms(MAX_RESULT_RETENTION_MS).unwrap();
        assert_eq!(ok.result_retention_ms(), MAX_RESULT_RETENTION_MS);
    }

    #[test]
    fn network_rules_must_be_allowed_by_the_deployment() {
        let mut p = SandboxPolicy::default();
        p.network_allow.push(rule("api.example.com", 443));
        let inside = NetworkPolicy::Allow(vec![rule("api.example.com", 443)]);
        assert!(p.check_network(&inside).is_ok());
        assert!(p.check_network(&NetworkPolicy::Deny).is_ok());
        // Same host, other port; and a rule after an allowed one.
        for outside in [
            NetworkPolicy::Allow(vec![rule("api.example.com", 80)]),
            NetworkPolicy::Allow(vec![
                rule("api.example.com", 443),
                rule("evil.example.net", 443),
            ]),
        ] {
            let err = p.check_network(&outside).unwrap_err();
            assert_eq!(err.full_code(), "policy_denied.network_rule");
        }
        let err = p
            .check_network(&NetworkPolicy::Allow(vec![
                rule("api.example.com", 443),
                rule("secret-host.example.net", 443),
            ]))
            .unwrap_err();
        assert_eq!(
            err,
            SandboxError::PolicyDenied(PolicyReason::NetworkRule {
                index: 1,
                problem: RuleProblem::NotInPolicy
            })
        );
        assert!(!format!("{err:?} {err}").contains("secret-host"));
    }

    #[test]
    fn default_policy_refuses_every_allow_request() {
        let p = SandboxPolicy::default();
        let req = create(NetworkPolicy::Allow(vec![rule("a.example.com", 443)]));
        assert_eq!(
            p.check_create(&req).unwrap_err().full_code(),
            "policy_denied.network_rule"
        );
        assert!(p.check_create(&create(NetworkPolicy::Deny)).is_ok());
    }

    #[test]
    fn check_create_still_applies_the_pure_validation() {
        let mut p = SandboxPolicy::default();
        p.ceiling.wall_time_ms = 10;
        let err = p.check_create(&create(NetworkPolicy::Deny)).unwrap_err();
        assert_eq!(err.full_code(), "policy_denied.limit_above_ceiling");
    }

    #[test]
    fn attempt_age_is_checked_against_the_retention() {
        let p = SandboxPolicy::default();
        let day = DEFAULT_RESULT_RETENTION_MS;
        let now = 10 * day;
        assert!(p.check_attempt_age(now, now).is_ok());
        assert!(
            p.check_attempt_age(now, now - day).is_ok(),
            "exactly at the limit"
        );
        let err = p.check_attempt_age(now, now - day - 1).unwrap_err();
        assert_eq!(err.full_code(), "policy_denied.attempt_too_old");
        // A start in the future is clock skew, not age.
        assert!(p.check_attempt_age(now, now + 5).is_ok());
        assert!(p.check_attempt_age(0, 0).is_ok());
    }
}
