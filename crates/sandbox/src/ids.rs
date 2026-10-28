//! Identity, execution context, idempotency key and sandbox handle.

use ailu_graph_core::{NodeId, RunId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Opaque tenant identifier, provided by the host and never by an agent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque identifier of one sandbox, assigned by the backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SandboxId(pub String);

impl SandboxId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque identifier of an isolation domain, assigned by the backend. A domain is never
/// shared between tenants, nor reused for another tenant after `destroy`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IsolationDomain(pub String);

impl IsolationDomain {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a backend returns from `create`. It cannot build a [`SandboxHandle`] itself: the
/// guard derives the handle from this value and the tenant of the execution context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawCreated {
    pub sandbox_id: SandboxId,
    pub isolation_domain: IsolationDomain,
}

/// Execution context, supplied by the host and never by an agent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxContext {
    pub tenant_id: TenantId,
    pub run_id: RunId,
    pub node_id: NodeId,
    /// Attempt of the node, 1-based.
    pub attempt: u32,
}

/// The operation an idempotency key belongs to. The numeric value is part of the key format
/// and is fixed by the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStep {
    Create,
    Exec,
    Collect,
    Destroy,
}

impl SandboxStep {
    /// Fixed step number: create = 0, exec = 1, collect = 2, destroy = 3.
    pub const fn number(self) -> u32 {
        match self {
            SandboxStep::Create => 0,
            SandboxStep::Exec => 1,
            SandboxStep::Collect => 2,
            SandboxStep::Destroy => 3,
        }
    }
}

/// Idempotency key of one sandbox operation.
///
/// Canonical form: every segment (tenant, run, node, attempt, step) is prefixed by its byte
/// length (`"6:tenant1|3:run|…"`), then the whole string is hashed with SHA-256 and rendered
/// as lowercase hex. Segments are never concatenated bare, so two different contexts cannot
/// collide through a separator character.
///
/// Not deserializable and not constructible outside this crate: a backend cannot forge one.
///
/// ```compile_fail,E0624
/// use ailu_sandbox::{IdempotencyKey, SandboxContext, SandboxStep};
/// fn forge(ctx: &SandboxContext) -> IdempotencyKey {
///     IdempotencyKey::new(ctx, SandboxStep::Exec)
/// }
/// ```
///
/// ```compile_fail,E0451
/// use ailu_sandbox::{IdempotencyKey, SandboxStep, TenantId};
/// let _ = IdempotencyKey {
///     tenant_id: TenantId::new("t"),
///     step: SandboxStep::Exec,
///     digest: String::new(),
/// };
/// ```
///
/// ```compile_fail,E0277
/// let _ = serde_json::from_str::<ailu_sandbox::IdempotencyKey>("\"abc\"");
/// ```
///
/// What a backend *can* do is read a key it was given:
///
/// ```
/// fn read(key: &ailu_sandbox::IdempotencyKey) -> &str {
///     key.as_str()
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct IdempotencyKey {
    tenant_id: TenantId,
    step: SandboxStep,
    digest: String,
}

impl IdempotencyKey {
    pub(crate) fn new(ctx: &SandboxContext, step: SandboxStep) -> Self {
        let attempt = ctx.attempt.to_string();
        let step_number = step.number().to_string();
        let segments = [
            ctx.tenant_id.as_str(),
            ctx.run_id.as_str(),
            ctx.node_id.as_str(),
            attempt.as_str(),
            step_number.as_str(),
        ];
        let canonical = segments
            .iter()
            .map(|s| format!("{}:{}", s.len(), s))
            .collect::<Vec<_>>()
            .join("|");
        let digest = Sha256::digest(canonical.as_bytes());
        Self {
            tenant_id: ctx.tenant_id.clone(),
            step,
            digest: hex(&digest),
        }
    }

    /// Tenant the key was derived for.
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Operation the key belongs to.
    pub fn step(&self) -> SandboxStep {
        self.step
    }

    /// Lowercase hex SHA-256 of the canonical form.
    pub fn as_str(&self) -> &str {
        &self.digest
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// Reference to a live sandbox. Only buildable inside this crate.
///
/// ```compile_fail,E0624
/// use ailu_sandbox::{SandboxHandle, SandboxId, TenantId};
/// let _ = SandboxHandle::new(SandboxId::new("s"), TenantId::new("t"));
/// ```
///
/// ```compile_fail,E0451
/// use ailu_sandbox::{SandboxHandle, SandboxId, TenantId};
/// let _ = SandboxHandle {
///     sandbox_id: SandboxId::new("s"),
///     tenant_id: TenantId::new("t"),
/// };
/// ```
///
/// ```compile_fail,E0277
/// let _ = serde_json::from_str::<ailu_sandbox::SandboxHandle>("{}");
/// ```
///
/// Reading one is fine:
///
/// ```
/// fn tenant(h: &ailu_sandbox::SandboxHandle) -> &str {
///     h.tenant_id().as_str()
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxHandle {
    sandbox_id: SandboxId,
    tenant_id: TenantId,
}

impl SandboxHandle {
    pub(crate) fn new(sandbox_id: SandboxId, tenant_id: TenantId) -> Self {
        Self {
            sandbox_id,
            tenant_id,
        }
    }

    /// Build the handle from a backend's `create` result and the context's tenant.
    pub(crate) fn from_created(created: &RawCreated, tenant_id: &TenantId) -> Self {
        Self::new(created.sandbox_id.clone(), tenant_id.clone())
    }

    pub fn sandbox_id(&self) -> &SandboxId {
        &self.sandbox_id
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tenant: &str, run: &str, node: &str, attempt: u32) -> SandboxContext {
        SandboxContext {
            tenant_id: TenantId::new(tenant),
            run_id: RunId::from(run),
            node_id: NodeId::from(node),
            attempt,
        }
    }

    #[test]
    fn step_numbers_are_fixed() {
        assert_eq!(SandboxStep::Create.number(), 0);
        assert_eq!(SandboxStep::Exec.number(), 1);
        assert_eq!(SandboxStep::Collect.number(), 2);
        assert_eq!(SandboxStep::Destroy.number(), 3);
    }

    #[test]
    fn key_is_deterministic_hex_sha256() {
        let a = IdempotencyKey::new(&ctx("t", "r", "n", 1), SandboxStep::Exec);
        let b = IdempotencyKey::new(&ctx("t", "r", "n", 1), SandboxStep::Exec);
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 64);
        assert!(a
            .as_str()
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    }

    #[test]
    fn key_matches_the_documented_canonical_form() {
        let key = IdempotencyKey::new(&ctx("tenant1", "run", "n", 2), SandboxStep::Collect);
        let expected = Sha256::digest(b"7:tenant1|3:run|1:n|1:2|1:2");
        assert_eq!(key.as_str(), hex(&expected));
    }

    #[test]
    fn every_input_changes_the_key() {
        let base = IdempotencyKey::new(&ctx("t", "r", "n", 1), SandboxStep::Exec);
        let others = [
            IdempotencyKey::new(&ctx("t2", "r", "n", 1), SandboxStep::Exec),
            IdempotencyKey::new(&ctx("t", "r2", "n", 1), SandboxStep::Exec),
            IdempotencyKey::new(&ctx("t", "r", "n2", 1), SandboxStep::Exec),
            IdempotencyKey::new(&ctx("t", "r", "n", 2), SandboxStep::Exec),
            IdempotencyKey::new(&ctx("t", "r", "n", 1), SandboxStep::Create),
        ];
        for other in others {
            assert_ne!(base.as_str(), other.as_str());
        }
    }

    #[test]
    fn separator_characters_cannot_make_keys_collide() {
        // Bare concatenation with `|` would make these identical.
        let a = IdempotencyKey::new(&ctx("a|b", "c", "n", 1), SandboxStep::Exec);
        let b = IdempotencyKey::new(&ctx("a", "b|c", "n", 1), SandboxStep::Exec);
        assert_ne!(a.as_str(), b.as_str());
        // Length prefixes also separate digit-looking segments.
        let c = IdempotencyKey::new(&ctx("1:a", "b", "n", 1), SandboxStep::Exec);
        let d = IdempotencyKey::new(&ctx("1", "a|1:b", "n", 1), SandboxStep::Exec);
        assert_ne!(c.as_str(), d.as_str());
    }

    #[test]
    fn known_answer_vectors_are_frozen() {
        // Computed independently from the canonical strings; changing the format breaks
        // idempotency across versions, so these values must never change.
        let vectors = [
            (
                ctx("tenant1", "run", "n", 2),
                SandboxStep::Collect,
                "20f06982eaf594b908a18f8fa35560d523fc371b1f74fd4cfb5ba8c64d00246a",
            ),
            (
                ctx("\u{e9}", "r", "n", 1),
                SandboxStep::Exec,
                "0b840c66ca6d46884209298f08c4a756bbbc272d6333e8964657a6bca6a1523d",
            ),
            (
                ctx("", "", "", 1),
                SandboxStep::Create,
                "d47f7981db41b1849694a447851e4ec4d38f9fbf73222744a0a49dda79d135a4",
            ),
        ];
        for (context, step, expected) in vectors {
            assert_eq!(IdempotencyKey::new(&context, step).as_str(), expected);
        }
    }

    #[test]
    fn lengths_are_counted_in_bytes_not_characters() {
        // "é" is two bytes in UTF-8, so a one-byte "e" cannot be confused with it.
        let accented = IdempotencyKey::new(&ctx("\u{e9}", "r", "n", 1), SandboxStep::Exec);
        let plain = IdempotencyKey::new(&ctx("e", "r", "n", 1), SandboxStep::Exec);
        assert_ne!(accented.as_str(), plain.as_str());
    }

    #[test]
    fn empty_segments_still_give_distinct_keys() {
        let a = IdempotencyKey::new(&ctx("", "r", "n", 1), SandboxStep::Exec);
        let b = IdempotencyKey::new(&ctx("r", "", "n", 1), SandboxStep::Exec);
        let c = IdempotencyKey::new(&ctx("", "", "rn", 1), SandboxStep::Exec);
        assert_ne!(a.as_str(), b.as_str());
        assert_ne!(a.as_str(), c.as_str());
        assert_ne!(b.as_str(), c.as_str());
    }

    #[test]
    fn key_remembers_its_tenant_and_step() {
        let key = IdempotencyKey::new(&ctx("t", "r", "n", 1), SandboxStep::Destroy);
        assert_eq!(key.tenant_id().as_str(), "t");
        assert_eq!(key.step(), SandboxStep::Destroy);
    }

    #[test]
    fn context_rejects_unknown_fields() {
        let json = r#"{"tenantId":"t","runId":"r","nodeId":"n","attempt":1,"extra":1}"#;
        assert!(serde_json::from_str::<SandboxContext>(json).is_err());
        let ok = r#"{"tenantId":"t","runId":"r","nodeId":"n","attempt":1}"#;
        assert_eq!(
            serde_json::from_str::<SandboxContext>(ok).unwrap(),
            ctx("t", "r", "n", 1)
        );
    }

    #[test]
    fn handle_takes_the_tenant_of_the_context_not_the_backend() {
        let created = RawCreated {
            sandbox_id: SandboxId::new("sb-2"),
            isolation_domain: IsolationDomain::new("dom-1"),
        };
        let handle = SandboxHandle::from_created(&created, &TenantId::new("t"));
        assert_eq!(handle.sandbox_id().as_str(), "sb-2");
        assert_eq!(handle.tenant_id().as_str(), "t");
    }

    #[test]
    fn raw_created_round_trips_and_rejects_extra_fields() {
        let json = r#"{"sandboxId":"sb","isolationDomain":"d"}"#;
        let created: RawCreated = serde_json::from_str(json).unwrap();
        assert_eq!(created.isolation_domain.as_str(), "d");
        let extra = r#"{"sandboxId":"sb","isolationDomain":"d","tenantId":"x"}"#;
        assert!(serde_json::from_str::<RawCreated>(extra).is_err());
    }

    #[test]
    fn handle_exposes_its_identity() {
        let h = SandboxHandle::new(SandboxId::new("sb-1"), TenantId::new("t"));
        assert_eq!(h.sandbox_id().as_str(), "sb-1");
        assert_eq!(h.tenant_id().as_str(), "t");
    }
}
