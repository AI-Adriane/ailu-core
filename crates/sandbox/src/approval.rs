//! Approval binding: the exact call an approver is asked about, its call key, and the grant
//! the guard accepts for it.
//!
//! The call key borrows the *name* of the ADR 0051 convention, `<name>#<sha256(...)>`, with the
//! name `sandbox.exec`. It is not the ADR 0051 `callInput`: the hash does not cover canonical
//! JSON of the call's input but a digest with length-prefixed fields over the context
//! identity (tenant, run, node), the fingerprint of the `create` request and the fingerprint
//! of the `exec` request, so a verifier cannot recompute it from a JSON input the way
//! ADR 0051 verifiers do. Changing any argument, the standard input, the
//! network list, the granted secret names or the limits therefore changes the key, and a
//! grant for the old key unlocks nothing. The attempt number is not part of the key: a
//! resume re-runs the node with the same attempt, and a retry of the same call is the same
//! call.
//!
//! The caller presents only an [`ApprovalClaim`]; a [`GrantVerifier`] (the host's approval
//! engine) establishes the approval, and the guard trusts nothing else.
//!
//! The subject shown to the approver is what the approver signs, whole: the arguments, the
//! working directory, the network rules, the names of the environment variables and the
//! references of the secrets (never values), the input destinations, the limits and the
//! `ttl`. The standard input is only a size. Nothing is truncated or marked as hidden: a
//! call that cannot be shown in full (an argument or a field over 256 characters, more than
//! 32 arguments) or whose text could mislead a reader (control, escape, bidirectional or
//! invisible format characters) is refused with `unreviewable_call` / `unsafe_text` before
//! any approval is asked. `RequestBounds` may therefore be wider than what can be reviewed;
//! the review limits apply on top of them. Known secret values, when the host supplies them,
//! are redacted from the arguments by known value (the same redaction as the collected
//! output; it is not the pattern-based redaction of ADR 0032). The call key is computed on
//! the full fingerprints.
//!
//! The error the guard returns when an approval is missing carries only a minimal subject
//! (the call key and counts); the full subject comes from [`ApprovalRequest`], on the host
//! side.

use crate::collected::{redact_text, ResolvedSecrets};
use crate::digest::Canon;
use crate::error::{ApprovalSubject, PolicyReason, SandboxError};
use crate::ids::{SandboxContext, TenantId};
use crate::request::{CreateRequest, ExecRequest};
use ailu_graph_core::RunId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Name of the gated call, the left side of the call key.
pub const EXEC_CALL_NAME: &str = "sandbox.exec";

/// What the caller presents to prove an approval exists: the approval's identifier and the
/// call key it says it covers. It proves nothing by itself; the guard hands it to the
/// injected [`GrantVerifier`] and trusts only what the verifier returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalClaim {
    pub approval_id: String,
    pub call_key: String,
}

/// An approval as established by the verifier, not by the caller. The host's approval engine
/// (or its signed attestation) is the expected implementation of [`GrantVerifier`]: it looks
/// the approval up, checks its decision and signature, and fills this record from what it
/// holds.
///
/// The guard compares `tenant_id`, `run_id` and the principals as given, so the verifier must
/// return them in one canonical form (the same spelling for the same tenant, run and
/// principal every time); the guard does not normalize identities beyond comparing the
/// requester and the approver trimmed and ignoring ASCII case.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedGrant {
    pub approval_id: String,
    pub call_key: String,
    pub tenant_id: TenantId,
    pub run_id: RunId,
    pub requested_by: String,
    pub approved_by: String,
    /// Payload hash of the approval record that authorized the call, as the verifier read it:
    /// 64 lowercase hexadecimal characters, or the grant is refused. The guard journals it as
    /// an identifier; the execution proof will carry it later to link the approval chain.
    pub approval_payload_hash: String,
}

impl VerifiedGrant {
    pub fn approval_payload_hash(&self) -> &str {
        &self.approval_payload_hash
    }
}

impl std::fmt::Debug for VerifiedGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedGrant")
            .field("approval_id", &self.approval_id)
            .field("call_key", &self.call_key)
            .field("tenant_id", &self.tenant_id)
            .field("run_id", &self.run_id)
            .field("requested_by", &"..")
            .field("approved_by", &"..")
            .field("approval_payload_hash", &"..")
            .finish()
    }
}

/// Why a claim was not verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    /// No such approval, not approved, or not valid: the guard answers `ApprovalRequired`.
    #[error("approval not established")]
    Unknown,
    /// The verifier could not answer: the guard fails closed with a retriable `Unavailable`.
    #[error("approval verifier unavailable")]
    Unavailable,
}

/// Port to the host's approval system. The guard never believes a claim on its own word.
///
/// Identities must be canonical on the verifier's side: the tenant, the run and the
/// principals of a [`VerifiedGrant`] are compared as given. The expected implementation is
/// the host's approval engine (or its signed attestation).
#[async_trait]
pub trait GrantVerifier: Send + Sync {
    async fn verify(
        &self,
        ctx: &SandboxContext,
        claim: &ApprovalClaim,
    ) -> Result<VerifiedGrant, GrantError>;
}

/// In-memory verifier for tests and single-process use: a table of approvals the host
/// registered. Not an approval engine.
#[derive(Default)]
pub struct InMemoryGrantVerifier {
    grants: Mutex<HashMap<String, VerifiedGrant>>,
    unavailable: AtomicBool,
}

impl InMemoryGrantVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an approval, replacing one with the same identifier.
    pub fn approve(&self, grant: VerifiedGrant) {
        if let Ok(mut grants) = self.grants.lock() {
            grants.insert(grant.approval_id.clone(), grant);
        }
    }

    /// Make every `verify` answer [`GrantError::Unavailable`] (or stop doing so).
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }
}

#[async_trait]
impl GrantVerifier for InMemoryGrantVerifier {
    async fn verify(
        &self,
        _ctx: &SandboxContext,
        claim: &ApprovalClaim,
    ) -> Result<VerifiedGrant, GrantError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(GrantError::Unavailable);
        }
        let grants = self.grants.lock().map_err(|_| GrantError::Unavailable)?;
        grants
            .get(&claim.approval_id)
            .cloned()
            .ok_or(GrantError::Unknown)
    }
}

/// The outcome of verifying a claim: `None` when the caller presented none.
pub(crate) type Verification = Option<Result<VerifiedGrant, GrantError>>;

/// Verify `claim` (if any) with `verifier`. A verifier answer that does not name the claimed
/// approval and call is treated as unknown.
pub(crate) async fn verify_claim(
    verifier: &dyn GrantVerifier,
    ctx: &SandboxContext,
    claim: Option<&ApprovalClaim>,
) -> Verification {
    let claim = claim?;
    Some(match verifier.verify(ctx, claim).await {
        Ok(v) if v.approval_id == claim.approval_id && v.call_key == claim.call_key => Ok(v),
        Ok(_) => Err(GrantError::Unknown),
        Err(e) => Err(e),
    })
}

/// What a node files with the approval engine before it creates anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    /// `sandbox.exec#<sha256>` of the canonical call.
    pub call_key: String,
    /// Masked description of the call, to show to the approver.
    pub subject: ApprovalSubject,
}

/// Digest of everything a `create` request asks for.
pub(crate) fn create_fingerprint(req: &CreateRequest) -> String {
    let mut c = Canon::new("sandbox.create.v1");
    c.str("profile", &req.profile)
        .json("limits", &req.limits)
        .json("network", &req.network)
        .num("ttlMs", req.ttl_ms)
        .num("inputs", req.inputs.len() as u64);
    for input in &req.inputs {
        c.json("input", input);
    }
    c.num("env", req.env.len() as u64);
    for grant in &req.env {
        c.str("envName", &grant.name);
        c.str("envRef", &grant.secret_ref.0);
    }
    c.finish()
}

/// Digest of an `exec` request. The standard input is hashed as it is written and never
/// kept.
pub(crate) fn exec_fingerprint(req: &ExecRequest) -> String {
    let mut c = Canon::new("sandbox.exec.v1");
    c.num("argc", req.argv.len() as u64);
    for arg in &req.argv {
        c.str("arg", arg);
    }
    match &req.cwd {
        Some(cwd) => c.str("cwd", cwd.as_str()),
        None => c.bytes("cwd", b"\0none"),
    };
    match &req.stdin {
        Some(stdin) => c.bytes("stdin", stdin),
        None => c.bytes("stdin", b"\0none"),
    };
    c.finish()
}

/// The call key of the call made of one `create` and one `exec` in one context.
pub(crate) fn call_key(ctx: &SandboxContext, create_fp: &str, exec_fp: &str) -> String {
    let mut c = Canon::new("sandbox.call.v1");
    c.str("tenant", ctx.tenant_id.as_str())
        .str("run", ctx.run_id.as_str())
        .str("node", ctx.node_id.as_str())
        .str("create", create_fp)
        .str("exec", exec_fp);
    format!("{EXEC_CALL_NAME}#{}", c.finish())
}

/// Most arguments an approver is asked to read.
pub const MAX_REVIEW_ARGS: usize = 32;
/// Longest argument or displayed field an approver is asked to read, in characters.
pub const MAX_REVIEW_CHARS: usize = 256;

fn unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}'
                | '\u{2029}'
                // invisible format characters
                | '\u{00AD}'
                | '\u{034F}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200D}'
                | '\u{2060}'..='\u{2064}'
                | '\u{206A}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{E0000}'..='\u{E007F}'
        )
}

fn refuse(reason: PolicyReason) -> SandboxError {
    SandboxError::PolicyDenied(reason)
}

/// Check one displayed text: no character that could mislead a reader, and short enough to be
/// shown whole.
fn check_text(text: &str) -> Result<(), SandboxError> {
    if text.chars().any(unsafe_char) {
        return Err(refuse(PolicyReason::UnsafeText));
    }
    if text.chars().count() > MAX_REVIEW_CHARS {
        return Err(refuse(PolicyReason::UnreviewableCall));
    }
    Ok(())
}

/// Refuse a `create` request whose displayed fields could not be shown whole or safely.
pub(crate) fn check_create_reviewable(req: &CreateRequest) -> Result<(), SandboxError> {
    check_text(&req.profile)?;
    if let crate::request::NetworkPolicy::Allow(rules) = &req.network {
        for rule in rules {
            check_text(&rule.host)?;
        }
    }
    for input in &req.inputs {
        check_text(input.dest.as_str())?;
    }
    for grant in &req.env {
        check_text(&grant.name)?;
        check_text(&grant.secret_ref.0)?;
    }
    Ok(())
}

/// Refuse an `exec` request that cannot be read whole: too many arguments, an argument or the
/// working directory over the length limit, or misleading characters.
pub(crate) fn check_exec_reviewable(req: &ExecRequest) -> Result<(), SandboxError> {
    if req.argv.len() > MAX_REVIEW_ARGS {
        return Err(refuse(PolicyReason::UnreviewableCall));
    }
    for arg in &req.argv {
        check_text(arg)?;
    }
    if let Some(cwd) = &req.cwd {
        check_text(cwd.as_str())?;
    }
    Ok(())
}

/// Description of a `create` request for the approver: everything the call asks for except
/// secret values, which the guard never holds (only names and references are shown).
pub(crate) fn create_subject(req: &CreateRequest) -> Value {
    json!({
        "profile": req.profile,
        "limits": req.limits,
        "network": req.network,
        "ttlMs": req.ttl_ms,
        "inputs": req.inputs.iter().map(|input| json!({
            "artifact": input.artifact,
            "dest": input.dest.as_str(),
        })).collect::<Vec<_>>(),
        "env": req.env.iter().map(|grant| json!({
            "name": grant.name,
            "secretRef": grant.secret_ref.0,
        })).collect::<Vec<_>>(),
    })
}

/// Description of an `exec` request for the approver: every argument (redacted by known
/// secret value), the working directory, and the standard input as a size only.
pub(crate) fn exec_subject(req: &ExecRequest, secrets: &ResolvedSecrets) -> Value {
    json!({
        "argv": req.argv.iter().map(|arg| redact_text(arg, secrets)).collect::<Vec<_>>(),
        "cwd": req.cwd.as_ref().map(|c| c.as_str()),
        "stdin": req.stdin.as_ref().map(|s| json!({ "bytes": s.len() })),
    })
}

/// Counts of a `create` request, kept by the guard for its minimal subject.
pub(crate) fn create_counts(req: &CreateRequest) -> Value {
    json!({
        "envCount": req.env.len(),
        "inputCount": req.inputs.len(),
        "networkRules": match &req.network {
            crate::request::NetworkPolicy::Deny => 0,
            crate::request::NetworkPolicy::Allow(rules) => rules.len(),
        },
    })
}

/// Counts of an `exec` request.
pub(crate) fn exec_counts(req: &ExecRequest) -> Value {
    json!({
        "argc": req.argv.len(),
        "stdinBytes": req.stdin.as_ref().map_or(0, Vec::len),
    })
}

/// The subject the guard puts in `ApprovalRequired`: the call key and counts, no argument, no
/// path, no name. The full subject comes from [`ApprovalRequest`] on the host side.
pub(crate) fn minimal_subject(
    ctx: &SandboxContext,
    call_key: &str,
    create_counts: &Value,
    exec_counts: &Value,
) -> ApprovalSubject {
    ApprovalSubject(json!({
        "description": EXEC_CALL_NAME,
        "callKey": call_key,
        "tenantId": ctx.tenant_id.as_str(),
        "runId": ctx.run_id.as_str(),
        "nodeId": ctx.node_id.as_str(),
        "create": create_counts,
        "exec": exec_counts,
    }))
}

/// Assemble the subject from its two masked halves.
pub(crate) fn subject(
    ctx: &SandboxContext,
    call_key: &str,
    create_part: &Value,
    exec_part: &Value,
) -> ApprovalSubject {
    ApprovalSubject(json!({
        "description": EXEC_CALL_NAME,
        "callKey": call_key,
        "tenantId": ctx.tenant_id.as_str(),
        "runId": ctx.run_id.as_str(),
        "nodeId": ctx.node_id.as_str(),
        "create": create_part,
        "exec": exec_part,
    }))
}

impl ApprovalRequest {
    /// Describe the call a node is about to make, without creating anything. The node files
    /// this with the approval engine and suspends; once approved, it passes the resulting
    /// [`ApprovalClaim`] to the guard.
    ///
    /// This only builds the description: it does not check the request. The guard's
    /// `request_approval` validates both requests first and then calls this.
    pub fn for_call(ctx: &SandboxContext, create: &CreateRequest, exec: &ExecRequest) -> Self {
        Self::for_call_redacting(ctx, create, exec, &ResolvedSecrets::new())
    }

    /// Like [`ApprovalRequest::for_call`], redacting from the shown arguments the secret
    /// values the host knows (the guard itself never holds them).
    pub fn for_call_redacting(
        ctx: &SandboxContext,
        create: &CreateRequest,
        exec: &ExecRequest,
        secrets: &ResolvedSecrets,
    ) -> Self {
        let key = call_key(ctx, &create_fingerprint(create), &exec_fingerprint(exec));
        let subject = subject(
            ctx,
            &key,
            &create_subject(create),
            &exec_subject(exec, secrets),
        );
        Self {
            call_key: key,
            subject,
        }
    }
}

/// The payload hash of an approval record: 64 lowercase hexadecimal characters.
fn is_payload_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Check a verified approval against the call key the guard expects.
///
/// - no claim, an unknown approval, one for another call, tenant or run, or one with an empty
///   field: `ApprovalRequired` with the masked subject;
/// - a verifier that could not answer: `Unavailable` (retriable);
/// - approver equal to requester (trimmed, ignoring ASCII case): `PolicyDenied(SelfApproval)`.
pub(crate) fn check_grant(
    verification: Option<&Result<VerifiedGrant, GrantError>>,
    ctx: &SandboxContext,
    expected_key: &str,
    subject: impl FnOnce() -> ApprovalSubject,
) -> Result<(), SandboxError> {
    let required = || SandboxError::ApprovalRequired { subject: subject() };
    let grant = match verification {
        None | Some(Err(GrantError::Unknown)) => return Err(required()),
        Some(Err(GrantError::Unavailable)) => {
            return Err(SandboxError::Unavailable { retriable: true })
        }
        Some(Ok(grant)) => grant,
    };
    if grant.call_key != expected_key
        || grant.tenant_id != ctx.tenant_id
        || grant.run_id != ctx.run_id
        || grant.approval_id.trim().is_empty()
        || grant.requested_by.trim().is_empty()
        || grant.approved_by.trim().is_empty()
        || !is_payload_hash(&grant.approval_payload_hash)
    {
        return Err(required());
    }
    if grant
        .approved_by
        .trim()
        .eq_ignore_ascii_case(grant.requested_by.trim())
    {
        return Err(SandboxError::PolicyDenied(PolicyReason::SelfApproval));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;
    use crate::path::LogicalPath;
    use crate::request::{EnvGrant, HostRule, NetworkPolicy, SecretRef};
    use ailu_graph_core::{NodeId, RunId};

    fn ctx(tenant: &str) -> SandboxContext {
        SandboxContext {
            tenant_id: crate::ids::TenantId::new(tenant),
            run_id: RunId::from("run"),
            node_id: NodeId::from("node"),
            attempt: 1,
        }
    }

    fn create() -> CreateRequest {
        CreateRequest {
            profile: "default".to_owned(),
            limits: Limits::default(),
            network: NetworkPolicy::Deny,
            inputs: vec![],
            env: vec![EnvGrant {
                name: "API_KEY".to_owned(),
                secret_ref: SecretRef("vault/key-1".to_owned()),
            }],
            ttl_ms: 1000,
        }
    }

    fn exec() -> ExecRequest {
        ExecRequest {
            argv: vec!["tool".to_owned(), "--flag".to_owned()],
            cwd: None,
            stdin: Some(b"input-data".to_vec()),
        }
    }

    fn key(c: &SandboxContext, cr: &CreateRequest, ex: &ExecRequest) -> String {
        ApprovalRequest::for_call(c, cr, ex).call_key
    }

    #[test]
    fn call_key_follows_the_adr_0051_convention() {
        let k = key(&ctx("t"), &create(), &exec());
        let (name, hash) = k.split_once('#').unwrap();
        assert_eq!(name, "sandbox.exec");
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(k, key(&ctx("t"), &create(), &exec()), "deterministic");
    }

    #[test]
    fn changing_anything_changes_the_call_key() {
        let base = key(&ctx("t"), &create(), &exec());
        let mut variants = Vec::new();
        let mut e = exec();
        e.argv[1] = "--other".to_owned();
        variants.push(key(&ctx("t"), &create(), &e));
        let mut e = exec();
        e.argv.push("extra".to_owned());
        variants.push(key(&ctx("t"), &create(), &e));
        let mut e = exec();
        e.stdin = Some(b"input-datb".to_vec());
        variants.push(key(&ctx("t"), &create(), &e));
        let mut e = exec();
        e.stdin = None;
        variants.push(key(&ctx("t"), &create(), &e));
        let mut e = exec();
        e.cwd = Some(LogicalPath::new("work").unwrap());
        variants.push(key(&ctx("t"), &create(), &e));
        let mut c = create();
        c.network = NetworkPolicy::Allow(vec![HostRule {
            host: "a.example.com".to_owned(),
            port: 443,
        }]);
        variants.push(key(&ctx("t"), &c, &exec()));
        let mut c = create();
        c.env[0].name = "OTHER".to_owned();
        variants.push(key(&ctx("t"), &c, &exec()));
        let mut c = create();
        c.env[0].secret_ref = SecretRef("vault/key-2".to_owned());
        variants.push(key(&ctx("t"), &c, &exec()));
        let mut c = create();
        c.limits.wall_time_ms += 1;
        variants.push(key(&ctx("t"), &c, &exec()));
        let mut c = create();
        c.profile = "other".to_owned();
        variants.push(key(&ctx("t"), &c, &exec()));
        variants.push(key(&ctx("t2"), &create(), &exec()));
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(*v, base, "variant {i} must change the key");
        }
        let mut sorted = variants.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            variants.len(),
            "variants are pairwise distinct"
        );
    }

    #[test]
    fn attempt_is_not_part_of_the_call_key() {
        let mut later = ctx("t");
        later.attempt = 3;
        assert_eq!(
            key(&ctx("t"), &create(), &exec()),
            key(&later, &create(), &exec())
        );
    }

    #[test]
    fn argument_boundaries_cannot_be_shifted() {
        let mut a = exec();
        a.argv = vec!["ab".to_owned(), "c".to_owned()];
        let mut b = exec();
        b.argv = vec!["a".to_owned(), "bc".to_owned()];
        assert_ne!(exec_fingerprint(&a), exec_fingerprint(&b));
        // Empty stdin is not the same as no stdin.
        let mut none = exec();
        none.stdin = None;
        let mut empty = exec();
        empty.stdin = Some(vec![]);
        assert_ne!(exec_fingerprint(&none), exec_fingerprint(&empty));
    }

    #[test]
    fn subject_shows_the_call_but_not_stdin() {
        let mut e = exec();
        e.argv = vec!["tool".to_owned(), "--token=s3cr3t-arg".to_owned()];
        e.stdin = Some(b"s3cr3t-stdin".to_vec());
        e.cwd = Some(LogicalPath::new("work").unwrap());
        let request = ApprovalRequest::for_call(&ctx("t"), &create(), &e);
        let text = serde_json::to_string(&request.subject).unwrap();
        assert!(!text.contains("s3cr3t-stdin"), "stdin leaked: {text}");
        let v = &request.subject.0;
        assert_eq!(v["description"], "sandbox.exec");
        assert_eq!(v["callKey"], request.call_key.as_str());
        assert_eq!(v["exec"]["argv"][0], "tool");
        assert_eq!(v["exec"]["argv"][1], "--token=s3cr3t-arg");
        assert_eq!(v["exec"]["cwd"], "work");
        assert_eq!(v["exec"]["stdin"]["bytes"], 12);
        assert_eq!(v["exec"]["stdin"], json!({ "bytes": 12 }));
        assert_eq!(v["create"]["env"][0]["name"], "API_KEY");
        assert_eq!(v["create"]["env"][0]["secretRef"], "vault/key-1");
        // The network list is shown: the approver must see which hosts open up.
        let mut c = create();
        c.network = NetworkPolicy::Allow(vec![HostRule {
            host: "a.example.com".to_owned(),
            port: 443,
        }]);
        let shown = ApprovalRequest::for_call(&ctx("t"), &c, &exec()).subject.0;
        assert_eq!(
            shown["create"]["network"]["rules"][0]["host"],
            "a.example.com"
        );
    }

    fn grant(call_key: &str) -> VerifiedGrant {
        VerifiedGrant {
            approval_id: "apr-1".to_owned(),
            call_key: call_key.to_owned(),
            tenant_id: TenantId::new("t"),
            run_id: RunId::from("run"),
            requested_by: "agent".to_owned(),
            approved_by: "alice".to_owned(),
            approval_payload_hash: "ab".repeat(32),
        }
    }

    fn check(g: Option<&Result<VerifiedGrant, GrantError>>, key: &str) -> Result<(), SandboxError> {
        check_grant(g, &ctx("t"), key, || {
            ApprovalSubject(json!({"callKey": key}))
        })
    }

    #[test]
    fn grant_must_name_the_exact_call_tenant_and_run() {
        assert!(check(Some(&Ok(grant("k"))), "k").is_ok());
        let wrong_tenant = VerifiedGrant {
            tenant_id: TenantId::new("other"),
            ..grant("k")
        };
        let wrong_run = VerifiedGrant {
            run_id: RunId::from("other"),
            ..grant("k")
        };
        let unknown = Err(GrantError::Unknown);
        for bad in [
            None,
            Some(&unknown),
            Some(&Ok(grant("other"))),
            Some(&Ok(wrong_tenant)),
            Some(&Ok(wrong_run)),
        ] {
            let err = check(bad, "k").unwrap_err();
            assert_eq!(err.code(), "approval_required");
        }
        for broken in [
            VerifiedGrant {
                approval_id: " ".to_owned(),
                ..grant("k")
            },
            VerifiedGrant {
                approved_by: String::new(),
                ..grant("k")
            },
            VerifiedGrant {
                requested_by: String::new(),
                ..grant("k")
            },
        ] {
            assert_eq!(
                check(Some(&Ok(broken)), "k").unwrap_err().code(),
                "approval_required"
            );
        }
        let down = Err(GrantError::Unavailable);
        assert_eq!(
            check(Some(&down), "k").unwrap_err(),
            SandboxError::Unavailable { retriable: true }
        );
    }

    #[test]
    fn approver_must_differ_from_requester() {
        for approver in ["agent", " Agent ", "AGENT"] {
            let g = VerifiedGrant {
                approved_by: approver.to_owned(),
                ..grant("k")
            };
            let err = check(Some(&Ok(g)), "k").unwrap_err();
            assert_eq!(err.full_code(), "policy_denied.self_approval");
        }
    }

    #[tokio::test]
    async fn verify_claim_trusts_only_the_verifier() {
        let verifier = InMemoryGrantVerifier::new();
        verifier.approve(grant("k"));
        let c = ctx("t");
        let claim = |id: &str, key: &str| ApprovalClaim {
            approval_id: id.to_owned(),
            call_key: key.to_owned(),
        };
        let ok = verify_claim(&verifier, &c, Some(&claim("apr-1", "k"))).await;
        assert_eq!(ok, Some(Ok(grant("k"))));
        // A claim naming another call than the approval covers, or an unknown approval.
        for bad in [claim("apr-1", "other"), claim("apr-2", "k")] {
            let got = verify_claim(&verifier, &c, Some(&bad)).await;
            assert_eq!(got, Some(Err(GrantError::Unknown)));
        }
        assert_eq!(verify_claim(&verifier, &c, None).await, None);
        verifier.set_unavailable(true);
        let down = verify_claim(&verifier, &c, Some(&claim("apr-1", "k"))).await;
        assert_eq!(down, Some(Err(GrantError::Unavailable)));
    }

    #[test]
    fn claim_rejects_unknown_fields() {
        let json = r#"{"approvalId":"a","callKey":"k","approvedBy":"me"}"#;
        assert!(serde_json::from_str::<ApprovalClaim>(json).is_err());
    }

    #[test]
    fn verified_grant_debug_hides_the_principals() {
        let shown = format!("{:?}", grant("k"));
        assert!(
            !shown.contains("alice") && !shown.contains("agent"),
            "{shown}"
        );
        assert!(shown.contains("apr-1"));
    }
}
