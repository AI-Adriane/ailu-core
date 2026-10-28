//! Guard tests. The backend here is a minimal spy that counts and records its calls; it is a
//! test double, not a reference backend.

use super::*;
use crate::collected::{RawCollected, RawFile, ResolvedSecrets};
use crate::error::PolicyReason;
use crate::ids::RawCreated;
use crate::journal::{Appended, InMemoryJournal, JournalError};
use crate::limits::Limits;
use crate::outcome::DeclaredExit;
use crate::path::LogicalPath;
use crate::policy::{TenantQuota, DEFAULT_RESULT_RETENTION_MS};
use crate::request::{EnvGrant, HostRule, NetworkPolicy, SecretRef};
use crate::secret::ResolvedSecret;
use ailu_graph_core::{NodeId, RunId};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const NOW: u64 = 30 * 24 * 3600 * 1000;
const RUN_STARTED: u64 = NOW - 1000;

// ---- doubles ------------------------------------------------------------------------

struct ManualClock(AtomicU64);

impl SandboxClock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Spy {
    calls: Mutex<Vec<(SandboxStep, String)>>,
    created: Mutex<HashMap<String, RawCreated>>,
    next_id: AtomicU64,
    domains: Mutex<HashMap<String, String>>,
    exec_script: Mutex<VecDeque<ExecResult>>,
    collect_script: Mutex<VecDeque<Result<RawCollected, SandboxError>>>,
    destroy_fails: AtomicBool,
    echo_error: AtomicBool,
    yield_in_create: AtomicBool,
    exec_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Spy {
    fn count(&self, step: SandboxStep) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _)| *s == step)
            .count()
    }

    fn steps(&self) -> Vec<SandboxStep> {
        self.calls.lock().unwrap().iter().map(|(s, _)| *s).collect()
    }

    fn total(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn script_exec(&self, result: ExecResult) {
        self.exec_script.lock().unwrap().push_back(result);
    }

    fn script_collect(&self, result: Result<RawCollected, SandboxError>) {
        self.collect_script.lock().unwrap().push_back(result);
    }

    fn set_domain(&self, tenant: &str, domain: &str) {
        self.domains
            .lock()
            .unwrap()
            .insert(tenant.to_owned(), domain.to_owned());
    }

    fn log(&self, step: SandboxStep, key: &IdempotencyKey) {
        assert_eq!(key.step(), step, "the guard passes the step's own key");
        self.calls
            .lock()
            .unwrap()
            .push((step, key.as_str().to_owned()));
    }
}

struct Shared(Arc<Spy>);

#[async_trait]
impl SandboxBackend for Shared {
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        _req: CreateRequest,
        _cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        let spy = &self.0;
        spy.log(SandboxStep::Create, key);
        if spy.yield_in_create.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        let mut created = spy.created.lock().unwrap();
        if let Some(raw) = created.get(key.as_str()) {
            return Ok(raw.clone());
        }
        let n = spy.next_id.fetch_add(1, Ordering::SeqCst);
        let domain = spy
            .domains
            .lock()
            .unwrap()
            .get(ctx.tenant_id.as_str())
            .cloned()
            .unwrap_or_else(|| format!("dom-{}", ctx.tenant_id.as_str()));
        let raw = RawCreated {
            sandbox_id: SandboxId::new(format!("sb-{n}")),
            isolation_domain: IsolationDomain::new(domain),
        };
        created.insert(key.as_str().to_owned(), raw.clone());
        Ok(raw)
    }

    async fn exec(
        &self,
        _h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        _cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        self.0.log(SandboxStep::Exec, key);
        if let Some(hook) = self.0.exec_hook.lock().unwrap().take() {
            hook();
        }
        if self.0.echo_error.load(Ordering::SeqCst) {
            let stdin =
                String::from_utf8_lossy(req.stdin.as_deref().unwrap_or_default()).into_owned();
            return Err(SandboxError::Backend(format!(
                "cannot run {:?} with input {stdin}",
                req.argv
            )));
        }
        self.0
            .exec_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(outcome()))
    }

    async fn collect(
        &self,
        _h: &SandboxHandle,
        key: &IdempotencyKey,
        _req: CollectRequest,
        _cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        self.0.log(SandboxStep::Collect, key);
        self.0
            .collect_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(raw_collected(b"hello", b"")))
    }

    async fn destroy(&self, _h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        self.0.log(SandboxStep::Destroy, key);
        if self.0.destroy_fails.load(Ordering::SeqCst) {
            return Err(SandboxError::Unavailable { retriable: true });
        }
        Ok(())
    }
}

#[derive(Default)]
struct FlakyJournal {
    inner: InMemoryJournal,
    fail: AtomicBool,
}

#[async_trait]
impl SandboxJournal for FlakyJournal {
    async fn append(&self, event: SandboxEvent) -> Result<Appended, JournalError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(JournalError);
        }
        self.inner.append(event).await
    }
}

struct Fx {
    guard: GovernedSandbox<Shared>,
    spy: Arc<Spy>,
    journal: Arc<FlakyJournal>,
    clock: Arc<ManualClock>,
}

impl Fx {
    fn kinds(&self) -> Vec<SandboxEventKind> {
        self.journal.inner.events().iter().map(|e| e.kind).collect()
    }
}

fn fx() -> Fx {
    fx_with(SandboxPolicy::default())
}

fn fx_with(policy: SandboxPolicy) -> Fx {
    let spy = Arc::new(Spy::default());
    let journal = Arc::new(FlakyJournal::default());
    let clock = Arc::new(ManualClock(AtomicU64::new(NOW)));
    let guard = GovernedSandbox::new(
        Shared(Arc::clone(&spy)),
        policy,
        Arc::clone(&clock) as Arc<dyn SandboxClock>,
        Arc::clone(&journal) as Arc<dyn SandboxJournal>,
    );
    Fx {
        guard,
        spy,
        journal,
        clock,
    }
}

/// A second guard over the same backend and journal, as after a host restart.
fn restarted(fx: &Fx) -> GovernedSandbox<Shared> {
    GovernedSandbox::new(
        Shared(Arc::clone(&fx.spy)),
        SandboxPolicy::default(),
        Arc::clone(&fx.clock) as Arc<dyn SandboxClock>,
        Arc::clone(&fx.journal) as Arc<dyn SandboxJournal>,
    )
}

fn ctx_for(tenant: &str, node: &str) -> SandboxContext {
    SandboxContext {
        tenant_id: TenantId::new(tenant),
        run_id: RunId::from("run-1"),
        node_id: NodeId::from(node),
        attempt: 1,
    }
}

fn ctx() -> SandboxContext {
    ctx_for("tenant-a", "node-1")
}

fn create_req() -> CreateRequest {
    CreateRequest {
        profile: "default".to_owned(),
        limits: Limits::default(),
        network: NetworkPolicy::Deny,
        inputs: vec![],
        env: vec![],
        ttl_ms: 60_000,
    }
}

fn exec_req() -> ExecRequest {
    ExecRequest {
        argv: vec!["tool".to_owned(), "--token=s3cr3t-arg".to_owned()],
        cwd: None,
        stdin: Some(b"s3cr3t-stdin".to_vec()),
    }
}

fn other_exec_req() -> ExecRequest {
    ExecRequest {
        argv: vec!["tool".to_owned(), "--other".to_owned()],
        cwd: None,
        stdin: None,
    }
}

fn collect_req() -> CollectRequest {
    CollectRequest {
        patterns: vec!["out/*".to_owned()],
        max_files: 4,
        max_bytes: 1024,
    }
}

fn outcome() -> ExecOutcome {
    ExecOutcome {
        declared_exit: DeclaredExit::Code { code: 0 },
        truncated: false,
        truncation: vec![],
        declared_duration_ms: 3,
    }
}

fn raw_collected(stdout: &[u8], stderr: &[u8]) -> RawCollected {
    RawCollected {
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        declared_stdout_sha256: sha256_hex(stdout),
        declared_stderr_sha256: sha256_hex(stderr),
        files: vec![],
        declared_truncated: false,
        secrets: ResolvedSecrets::new(),
    }
}

fn grant_for(
    fx: &Fx,
    ctx: &SandboxContext,
    create: &CreateRequest,
    exec: &ExecRequest,
) -> ApprovalGrant {
    let request = fx.guard.request_approval(ctx, create, exec).unwrap();
    ApprovalGrant {
        approval_id: "apr-1".to_owned(),
        call_key: request.call_key,
        requested_by: "agent".to_owned(),
        approved_by: "alice".to_owned(),
    }
}

fn never() -> CancelToken {
    CancelToken::never()
}

fn cancelled() -> CancelToken {
    CancelToken::new(Arc::new(|| true))
}

/// Approve and create a sandbox for `exec_req()`.
async fn ready(fx: &Fx, ctx: &SandboxContext) -> (SandboxHandle, ApprovalGrant) {
    let grant = grant_for(fx, ctx, &create_req(), &exec_req());
    let handle = fx
        .guard
        .create(ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await
        .unwrap();
    (handle, grant)
}

async fn run(
    fx: &Fx,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
    grant: &ApprovalGrant,
) -> ExecResult {
    fx.guard
        .exec(ctx, handle, exec_req(), Some(grant), RUN_STARTED, &never())
        .await
}

fn code<T: std::fmt::Debug>(result: Result<T, SandboxError>) -> String {
    result.unwrap_err().full_code().to_owned()
}

// ---- validation, order, events ----------------------------------------------------------

#[tokio::test]
async fn requests_are_validated_before_the_backend_is_reached() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());

    let mut zero_ttl = create_req();
    zero_ttl.ttl_ms = 0;
    let mut above_ceiling = create_req();
    above_ceiling.limits.memory_bytes += 1;
    let mut off_list = create_req();
    off_list.network = NetworkPolicy::Allow(vec![HostRule {
        host: "api.example.com".to_owned(),
        port: 443,
    }]);
    for (request, expected) in [
        (zero_ttl, "policy_denied.ttl_zero"),
        (above_ceiling, "policy_denied.limit_above_ceiling"),
        (off_list, "policy_denied.network_rule"),
    ] {
        let result = fx
            .guard
            .create(&ctx, request, &exec_req(), Some(&grant), &never())
            .await;
        assert_eq!(code(result), expected);
    }
    let mut empty_argv = exec_req();
    empty_argv.argv.clear();
    let result = fx
        .guard
        .create(&ctx, create_req(), &empty_argv, Some(&grant), &never())
        .await;
    assert_eq!(code(result), "policy_denied.empty_argv");
    let mut bad_ctx = ctx.clone();
    bad_ctx.attempt = 0;
    let result = fx
        .guard
        .create(&bad_ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await;
    assert_eq!(code(result), "policy_denied.invalid_context");
    assert_eq!(fx.spy.total(), 0, "nothing reached the backend");

    // After a sandbox exists, invalid exec and collect requests stop at the guard too.
    let (handle, grant) = ready(&fx, &ctx).await;
    let mut big_stdin = exec_req();
    big_stdin.stdin = Some(vec![0; 64 * 1024 + 1]);
    let result = fx
        .guard
        .exec(
            &ctx,
            &handle,
            big_stdin,
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(result), "policy_denied.request_too_large");
    let mut nul = exec_req();
    nul.argv[1] = "a\0b".to_owned();
    let result = fx
        .guard
        .exec(&ctx, &handle, nul, Some(&grant), RUN_STARTED, &never())
        .await;
    assert_eq!(code(result), "policy_denied.nul_in_argument");
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let many = CollectRequest {
        patterns: vec!["a".to_owned(); 9],
        max_files: 1,
        max_bytes: 1,
    };
    let result = fx
        .guard
        .collect(&ctx, &handle, many, RUN_STARTED, &never())
        .await;
    assert_eq!(code(result), "policy_denied.request_too_large");
    let traversal = CollectRequest {
        patterns: vec!["../x".to_owned()],
        ..collect_req()
    };
    let result = fx
        .guard
        .collect(&ctx, &handle, traversal, RUN_STARTED, &never())
        .await;
    assert_eq!(code(result), "invalid_path");
    assert_eq!(
        fx.spy.steps(),
        vec![SandboxStep::Create, SandboxStep::Exec],
        "only the valid calls reached the backend"
    );
}

#[tokio::test]
async fn the_lifecycle_runs_in_order_and_each_transition_has_one_event() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    assert_eq!(
        handle.tenant_id(),
        &ctx.tenant_id,
        "the guard sets the tenant"
    );
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let collected = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(collected.stdout, b"hello");
    fx.guard.destroy(&ctx, &handle).await.unwrap();

    assert_eq!(
        fx.spy.steps(),
        vec![
            SandboxStep::Create,
            SandboxStep::Exec,
            SandboxStep::Collect,
            SandboxStep::Destroy
        ]
    );
    assert_eq!(
        fx.kinds(),
        vec![
            SandboxEventKind::SandboxCreated,
            SandboxEventKind::SandboxExecStarted,
            SandboxEventKind::SandboxExecCompleted,
            SandboxEventKind::SandboxCollected,
            SandboxEventKind::SandboxDestroyed,
        ]
    );
    let events = fx.journal.inner.events();
    assert_eq!(
        events[0].isolation_domain,
        Some(IsolationDomain::new("dom-tenant-a"))
    );
    assert_eq!(events[0].at_ms, NOW);
    assert!(events
        .iter()
        .all(|e| e.tenant_id == ctx.tenant_id && e.attempt == 1));
}

#[tokio::test]
async fn operations_out_of_order_are_invalid_state() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let early = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        early.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Created
        }
    );
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    // A different collect after Collected is refused; the same one replays.
    let other = CollectRequest {
        max_files: 2,
        ..collect_req()
    };
    let again = fx
        .guard
        .collect(&ctx, &handle, other, RUN_STARTED, &never())
        .await;
    assert_eq!(
        again.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Collected
        }
    );
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    let after = run(&fx, &ctx, &handle, &grant).await;
    assert_eq!(after.unwrap_err().full_code(), "invalid_state");
    let after = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        after.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Destroyed
        }
    );
}

#[tokio::test]
async fn a_second_exec_is_refused_and_never_reaches_the_backend() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let second = other_exec_req();
    let grant_two = grant_for(&fx, &ctx, &create_req(), &second);
    let refused = fx
        .guard
        .exec(
            &ctx,
            &handle,
            second,
            Some(&grant_two),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(
        refused.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Executed
        }
    );
    assert_eq!(fx.spy.count(SandboxStep::Exec), 1);
    assert!(fx.kinds().contains(&SandboxEventKind::SandboxDenied));
}

#[tokio::test]
async fn replaying_a_call_returns_the_recorded_result_without_the_backend() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let first = run(&fx, &ctx, &handle, &grant).await.unwrap();
    let collected = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    let before = fx.spy.total();
    let events = fx.journal.inner.len();

    let again = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await
        .unwrap();
    assert_eq!(again, handle);
    assert_eq!(run(&fx, &ctx, &handle, &grant).await.unwrap(), first);
    let collected_again = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(collected_again, collected);
    assert_eq!(fx.spy.total(), before, "no backend call on replay");
    assert_eq!(fx.journal.inner.len(), events, "no event written twice");
}

#[tokio::test]
async fn the_same_key_with_another_request_is_an_idempotency_conflict() {
    let fx = fx();
    let ctx = ctx();
    ready(&fx, &ctx).await;
    let mut changed = create_req();
    changed.ttl_ms = 61_000;
    let grant = grant_for(&fx, &ctx, &changed, &exec_req());
    let result = fx
        .guard
        .create(&ctx, changed, &exec_req(), Some(&grant), &never())
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::IdempotencyConflict);
    assert_eq!(fx.spy.count(SandboxStep::Create), 1);

    // While the outcome of an exec is unknown, another request is a conflict too.
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.spy
        .script_exec(Err(SandboxError::Unavailable { retriable: true }));
    let failed = run(&fx, &ctx, &handle, &grant).await;
    assert!(failed.unwrap_err().is_retriable());
    let other = other_exec_req();
    let other_grant = grant_for(&fx, &ctx, &create_req(), &other);
    let conflict = fx
        .guard
        .exec(
            &ctx,
            &handle,
            other,
            Some(&other_grant),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(conflict.unwrap_err(), SandboxError::IdempotencyConflict);
    assert_eq!(fx.spy.count(SandboxStep::Exec), 1);
}

// ---- tenants ---------------------------------------------------------------------------

#[tokio::test]
async fn another_tenants_handle_is_not_found() {
    let fx = fx();
    let owner = ctx();
    let intruder = ctx_for("tenant-b", "node-1");
    let (handle, grant) = ready(&fx, &owner).await;
    run(&fx, &owner, &handle, &grant).await.unwrap();
    let calls = fx.spy.total();

    let stolen = fx
        .guard
        .exec(
            &intruder,
            &handle,
            exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(stolen.unwrap_err(), SandboxError::NotFound);
    let stolen = fx
        .guard
        .collect(&intruder, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(stolen.unwrap_err(), SandboxError::NotFound);
    assert_eq!(
        fx.guard.destroy(&intruder, &handle).await.unwrap_err(),
        SandboxError::NotFound
    );
    // Same tenant, another node or run: not found either.
    let other_node = ctx_for("tenant-a", "node-2");
    let result = fx.guard.destroy(&other_node, &handle).await;
    assert_eq!(result.unwrap_err(), SandboxError::NotFound);
    assert_eq!(fx.spy.total(), calls, "the backend never saw these calls");
}

#[tokio::test]
async fn a_domain_declared_for_two_tenants_poisons_the_second_sandbox() {
    let fx = fx();
    fx.spy.set_domain("tenant-a", "shared");
    fx.spy.set_domain("tenant-b", "shared");
    let a = ctx();
    let b = ctx_for("tenant-b", "node-1");
    let (handle_a, _) = ready(&fx, &a).await;

    let grant_b = grant_for(&fx, &b, &create_req(), &exec_req());
    let result = fx
        .guard
        .create(&b, create_req(), &exec_req(), Some(&grant_b), &never())
        .await;
    assert!(matches!(
        result.unwrap_err(),
        SandboxError::IsolationViolation { .. }
    ));
    // The guard tore the poisoned sandbox down itself; tenant A's is untouched.
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 1);
    assert!(fx.guard.destroy(&a, &handle_a).await.is_ok());
    // Replaying the refused create gives the same answer, with no new backend create.
    let creates = fx.spy.count(SandboxStep::Create);
    let again = fx
        .guard
        .create(&b, create_req(), &exec_req(), Some(&grant_b), &never())
        .await;
    assert_eq!(again.unwrap_err().code(), "isolation_violation");
    assert_eq!(fx.spy.count(SandboxStep::Create), creates);
    // Tenant B holds no slot for the destroyed poisoned sandbox.
    let inner = fx.guard.inner();
    assert_eq!(inner.tenants["tenant-b"].active, 0);
}

#[tokio::test]
async fn a_domain_is_never_reused_by_another_tenant_even_after_destroy() {
    let fx = fx();
    fx.spy.set_domain("tenant-a", "dom-1");
    fx.spy.set_domain("tenant-b", "dom-1");
    let a = ctx();
    let (handle, _) = ready(&fx, &a).await;
    fx.guard.destroy(&a, &handle).await.unwrap();

    let b = ctx_for("tenant-b", "node-1");
    let grant = grant_for(&fx, &b, &create_req(), &exec_req());
    let result = fx
        .guard
        .create(&b, create_req(), &exec_req(), Some(&grant), &never())
        .await;
    assert_eq!(result.unwrap_err().code(), "isolation_violation");

    // The same tenant may recycle its own domain.
    let a2 = ctx_for("tenant-a", "node-2");
    let grant = grant_for(&fx, &a2, &create_req(), &exec_req());
    fx.guard
        .create(&a2, create_req(), &exec_req(), Some(&grant), &never())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_reused_sandbox_id_is_an_isolation_violation() {
    let fx = fx();
    let a = ctx();
    ready(&fx, &a).await;
    // Make the backend hand out the id of an existing sandbox for a new key.
    fx.spy.next_id.store(0, Ordering::SeqCst);
    let b = ctx_for("tenant-b", "node-1");
    let grant = grant_for(&fx, &b, &create_req(), &exec_req());
    let result = fx
        .guard
        .create(&b, create_req(), &exec_req(), Some(&grant), &never())
        .await;
    assert_eq!(
        result.unwrap_err(),
        SandboxError::IsolationViolation {
            signal: SIGNAL_ID_REUSED.to_owned()
        }
    );
    // Nothing was destroyed: the id belongs to someone else.
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 0);
}

// ---- retention -------------------------------------------------------------------------

#[tokio::test]
async fn an_attempt_older_than_the_retention_is_refused_fail_closed() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let old = NOW - DEFAULT_RESULT_RETENTION_MS - 1;
    let calls = fx.spy.total();
    let result = fx
        .guard
        .exec(&ctx, &handle, exec_req(), Some(&grant), old, &never())
        .await;
    assert_eq!(
        result.unwrap_err(),
        SandboxError::PolicyDenied(PolicyReason::AttemptTooOld)
    );
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let calls_after_exec = fx.spy.total();
    let result = fx
        .guard
        .collect(&ctx, &handle, collect_req(), old, &never())
        .await;
    assert_eq!(code(result), "policy_denied.attempt_too_old");
    assert_eq!(fx.spy.total(), calls_after_exec);
    assert_eq!(calls_after_exec, calls + 1);
    // Exactly at the limit is accepted.
    let edge = NOW - DEFAULT_RESULT_RETENTION_MS;
    fx.guard
        .collect(&ctx, &handle, collect_req(), edge, &never())
        .await
        .unwrap();
}

// ---- admission --------------------------------------------------------------------------

fn quota_policy(max_concurrent: u32, max_creations: u32) -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota = TenantQuota {
        max_concurrent,
        max_creations,
        window_ms: 1000,
    };
    policy
}

async fn create_on(fx: &Fx, ctx: &SandboxContext) -> Result<SandboxHandle, SandboxError> {
    let grant = grant_for(fx, ctx, &create_req(), &exec_req());
    fx.guard
        .create(ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await
}

#[tokio::test]
async fn quota_is_refused_at_once_and_does_not_starve_other_tenants() {
    let fx = fx_with(quota_policy(1, 100));
    let a1 = ctx_for("tenant-a", "node-1");
    let a2 = ctx_for("tenant-a", "node-2");
    let handle = create_on(&fx, &a1).await.unwrap();
    let creates = fx.spy.count(SandboxStep::Create);

    let refused = create_on(&fx, &a2).await;
    assert_eq!(
        refused.unwrap_err(),
        SandboxError::QuotaExceeded {
            kind: QuotaKind::TenantConcurrency
        }
    );
    assert_eq!(
        fx.spy.count(SandboxStep::Create),
        creates,
        "refused before the backend"
    );
    // Another tenant is served while tenant A is at its ceiling.
    create_on(&fx, &ctx_for("tenant-b", "node-1"))
        .await
        .unwrap();
    // Destroying frees the slot.
    fx.guard.destroy(&a1, &handle).await.unwrap();
    create_on(&fx, &a2).await.unwrap();
    assert!(fx.kinds().contains(&SandboxEventKind::SandboxDenied));
}

#[tokio::test]
async fn creation_rate_is_limited_per_window() {
    let fx = fx_with(quota_policy(10, 2));
    for node in ["n1", "n2"] {
        let c = ctx_for("tenant-a", node);
        let handle = create_on(&fx, &c).await.unwrap();
        fx.guard.destroy(&c, &handle).await.unwrap();
    }
    let third = ctx_for("tenant-a", "n3");
    let refused = create_on(&fx, &third).await;
    assert_eq!(
        refused.unwrap_err(),
        SandboxError::QuotaExceeded {
            kind: QuotaKind::CreationRate
        }
    );
    create_on(&fx, &ctx_for("tenant-b", "n1")).await.unwrap();
    fx.clock.0.fetch_add(1000, Ordering::SeqCst);
    create_on(&fx, &third).await.unwrap();
}

#[tokio::test]
async fn a_failed_create_gives_its_slot_back() {
    let fx = fx_with(quota_policy(1, 100));
    let a = ctx();
    let cancelled_token = cancelled();
    let grant = grant_for(&fx, &a, &create_req(), &exec_req());
    let result = fx
        .guard
        .create(
            &a,
            create_req(),
            &exec_req(),
            Some(&grant),
            &cancelled_token,
        )
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::Cancelled);
    create_on(&fx, &a).await.unwrap();
}

#[tokio::test]
async fn concurrent_creates_with_one_key_share_one_sandbox_and_one_slot() {
    let fx = fx_with(quota_policy(2, 100));
    fx.spy.yield_in_create.store(true, Ordering::SeqCst);
    let a = ctx();
    let grant = grant_for(&fx, &a, &create_req(), &exec_req());
    let planned = exec_req();
    let token = never();
    let (one, two) = tokio::join!(
        fx.guard
            .create(&a, create_req(), &planned, Some(&grant), &token),
        fx.guard
            .create(&a, create_req(), &planned, Some(&grant), &token),
    );
    assert_eq!(one.unwrap(), two.unwrap());
    // One slot is held, so exactly one more sandbox fits under a ceiling of two.
    create_on(&fx, &ctx_for("tenant-a", "node-2"))
        .await
        .unwrap();
    let refused = create_on(&fx, &ctx_for("tenant-a", "node-3")).await;
    assert_eq!(refused.unwrap_err().code(), "quota_exceeded");
}

// ---- cancellation -----------------------------------------------------------------------

#[tokio::test]
async fn cancellation_is_checked_before_every_backend_call() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let token = cancelled();
    let result = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), Some(&grant), &token)
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::Cancelled);
    assert_eq!(fx.spy.total(), 0);

    let (handle, grant) = ready(&fx, &ctx).await;
    let calls = fx.spy.total();
    let result = fx
        .guard
        .exec(&ctx, &handle, exec_req(), Some(&grant), RUN_STARTED, &token)
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::Cancelled);
    assert_eq!(fx.spy.total(), calls);
    // The state did not move: the exec can still run.
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let calls = fx.spy.total();
    let result = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &token)
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::Cancelled);
    assert_eq!(fx.spy.total(), calls);
    // `destroy` is not cancellable: it takes no token and always reaches the backend.
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 1);
}

#[tokio::test]
async fn a_backend_that_reports_cancellation_leaves_the_exec_retryable() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.spy.script_exec(Err(SandboxError::Cancelled));
    assert_eq!(
        run(&fx, &ctx, &handle, &grant).await.unwrap_err(),
        SandboxError::Cancelled
    );
    let early = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        early.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Executing
        }
    );
    assert!(run(&fx, &ctx, &handle, &grant).await.is_ok());
    assert_eq!(fx.spy.count(SandboxStep::Exec), 2);
}

// ---- approval ---------------------------------------------------------------------------

#[tokio::test]
async fn nothing_is_created_without_an_approval() {
    let fx = fx();
    let ctx = ctx();
    let result = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), None, &never())
        .await;
    let SandboxError::ApprovalRequired { subject } = result.unwrap_err() else {
        panic!("expected ApprovalRequired");
    };
    assert_eq!(fx.spy.total(), 0, "no sandbox before the approval");
    // The subject is the one `request_approval` hands to the node.
    let request = fx
        .guard
        .request_approval(&ctx, &create_req(), &exec_req())
        .unwrap();
    assert_eq!(subject, request.subject);
    assert_eq!(subject.0["callKey"], request.call_key.as_str());
    // Asking again does not write the same refusal twice.
    let _ = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), None, &never())
        .await;
    let denied = fx
        .kinds()
        .iter()
        .filter(|k| **k == SandboxEventKind::SandboxDenied)
        .count();
    assert_eq!(denied, 1);
}

#[tokio::test]
async fn an_exec_without_a_grant_is_refused_and_does_not_run() {
    let fx = fx();
    let ctx = ctx();
    let (handle, _grant) = ready(&fx, &ctx).await;
    let result = fx
        .guard
        .exec(&ctx, &handle, exec_req(), None, RUN_STARTED, &never())
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
    assert_eq!(fx.spy.count(SandboxStep::Exec), 0);
}

#[tokio::test]
async fn changing_an_argument_invalidates_the_grant() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let mut changed = exec_req();
    changed.argv[1] = "--token=another".to_owned();
    let result = fx
        .guard
        .exec(&ctx, &handle, changed, Some(&grant), RUN_STARTED, &never())
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
    assert_eq!(fx.spy.count(SandboxStep::Exec), 0);
    // The same holds for create: a grant for another plan unlocks nothing.
    let other = ctx_for("tenant-a", "node-2");
    let stale = grant_for(&fx, &other, &create_req(), &other_exec_req());
    let result = fx
        .guard
        .create(&other, create_req(), &exec_req(), Some(&stale), &never())
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
    // A grant from another tenant, for the same request, unlocks nothing either.
    let foreign = grant_for(
        &fx,
        &ctx_for("tenant-b", "node-1"),
        &create_req(),
        &exec_req(),
    );
    let result = fx
        .guard
        .create(&other, create_req(), &exec_req(), Some(&foreign), &never())
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
}

#[tokio::test]
async fn a_new_grant_for_the_changed_call_unlocks_it() {
    let fx = fx();
    let ctx = ctx();
    let (handle, _) = ready(&fx, &ctx).await;
    let changed = other_exec_req();
    let grant = grant_for(&fx, &ctx, &create_req(), &changed);
    fx.guard
        .exec(&ctx, &handle, changed, Some(&grant), RUN_STARTED, &never())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_agent_cannot_approve_its_own_execution() {
    let fx = fx();
    let ctx = ctx();
    let mut grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    grant.approved_by = grant.requested_by.clone();
    let result = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await;
    assert_eq!(
        result.unwrap_err(),
        SandboxError::PolicyDenied(PolicyReason::SelfApproval)
    );
    assert_eq!(fx.spy.total(), 0);
    // And at exec, on a sandbox created with a good grant.
    let (handle, good) = ready(&fx, &ctx).await;
    let selfish = ApprovalGrant {
        approved_by: good.requested_by.clone(),
        ..good
    };
    let result = run(&fx, &ctx, &handle, &selfish).await;
    assert_eq!(
        result.unwrap_err().full_code(),
        "policy_denied.self_approval"
    );
    assert_eq!(fx.spy.count(SandboxStep::Exec), 0);
}

#[tokio::test]
async fn the_approval_subject_never_carries_stdin_arguments_or_secret_names() {
    let fx = fx();
    let ctx = ctx();
    let mut create = create_req();
    create.env.push(EnvGrant {
        name: "DB_PASSWORD".to_owned(),
        secret_ref: SecretRef("vault/prod/db".to_owned()),
    });
    let request = fx
        .guard
        .request_approval(&ctx, &create, &exec_req())
        .unwrap();
    let shown = serde_json::to_string(&request).unwrap();
    for hidden in ["s3cr3t", "tool", "DB_PASSWORD", "vault"] {
        assert!(!shown.contains(hidden), "{hidden} leaked in {shown}");
    }
    assert_eq!(request.subject.0["exec"]["stdin"]["bytes"], 12);
    assert_eq!(request.subject.0["exec"]["argv"]["count"], 2);
    // The error a node receives carries the same masked subject.
    let refused = fx
        .guard
        .create(&ctx, create, &exec_req(), None, &never())
        .await
        .unwrap_err();
    let wire = serde_json::to_string(&refused).unwrap();
    assert!(
        !wire.contains("s3cr3t") && !wire.contains("DB_PASSWORD"),
        "{wire}"
    );
}

#[test]
fn request_approval_validates_and_creates_nothing() {
    let fx = fx();
    let ctx = ctx();
    let mut empty = exec_req();
    empty.argv.clear();
    let err = fx
        .guard
        .request_approval(&ctx, &create_req(), &empty)
        .unwrap_err();
    assert_eq!(err.full_code(), "policy_denied.empty_argv");
    let mut zero = create_req();
    zero.ttl_ms = 0;
    assert!(fx.guard.request_approval(&ctx, &zero, &exec_req()).is_err());
    assert_eq!(fx.spy.total(), 0);
}

// ---- events and content -----------------------------------------------------------------

#[tokio::test]
async fn a_restarted_host_replaying_the_node_does_not_duplicate_events() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let first = run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    let recorded = fx.journal.inner.events();

    // A new guard (empty registry) replays the node against the same idempotent backend.
    let guard = restarted(&fx);
    let handle2 = guard
        .create(&ctx, create_req(), &exec_req(), Some(&grant), &never())
        .await
        .unwrap();
    assert_eq!(
        handle2, handle,
        "the backend's idempotence returns the same sandbox"
    );
    let again = guard
        .exec(
            &ctx,
            &handle2,
            exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    assert_eq!(again, first);
    guard
        .collect(&ctx, &handle2, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    guard.destroy(&ctx, &handle2).await.unwrap();
    assert_eq!(
        fx.journal.inner.events(),
        recorded,
        "same keys, same events: nothing re-emitted"
    );
}

#[tokio::test]
async fn events_hold_identifiers_and_digests_never_content() {
    let fx = fx();
    let ctx = ctx();
    let mut create = create_req();
    create.env.push(EnvGrant {
        name: "DB_PASSWORD".to_owned(),
        secret_ref: SecretRef("vault/prod/db".to_owned()),
    });
    let grant = grant_for(&fx, &ctx, &create, &exec_req());
    let handle = fx
        .guard
        .create(&ctx, create, &exec_req(), Some(&grant), &never())
        .await
        .unwrap();
    let mut raw = raw_collected(b"s3cr3t-out", b"");
    raw.secrets.push(ResolvedSecret::new("hunter2-value"));
    fx.spy.script_collect(Ok(raw));
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    // A refusal event as well.
    let _ = fx
        .guard
        .exec(&ctx, &handle, other_exec_req(), None, RUN_STARTED, &never())
        .await;
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    let wire = serde_json::to_string(&fx.journal.inner.events()).unwrap();
    for hidden in [
        "s3cr3t",
        "hunter2",
        "DB_PASSWORD",
        "vault",
        "--other",
        "tool",
        "hello",
    ] {
        assert!(!wire.contains(hidden), "{hidden} leaked in {wire}");
    }
    assert!(wire.contains("sandbox_exec_completed") && wire.contains("sandbox_collected"));
}

#[tokio::test]
async fn a_journal_that_cannot_record_fails_closed_and_the_call_can_be_retried() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.journal.fail.store(true, Ordering::SeqCst);
    let blocked = run(&fx, &ctx, &handle, &grant).await;
    assert_eq!(
        blocked.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );
    assert_eq!(fx.spy.count(SandboxStep::Exec), 0, "no event, no execution");
    fx.journal.fail.store(false, Ordering::SeqCst);
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    assert_eq!(fx.spy.count(SandboxStep::Exec), 1);

    // Failing after the exec: the result is kept and the retry replays it.
    let ctx2 = ctx_for("tenant-a", "node-2");
    let grant2 = grant_for(&fx, &ctx2, &create_req(), &exec_req());
    let handle2 = fx
        .guard
        .create(&ctx2, create_req(), &exec_req(), Some(&grant2), &never())
        .await
        .unwrap();
    let journal = Arc::clone(&fx.journal);
    *fx.spy.exec_hook.lock().unwrap() = Some(Box::new(move || {
        journal.fail.store(true, Ordering::SeqCst);
    }));
    let lost = run(&fx, &ctx2, &handle2, &grant2).await;
    assert_eq!(
        lost.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );
    let calls = fx.spy.count(SandboxStep::Exec);
    fx.journal.fail.store(false, Ordering::SeqCst);
    assert_eq!(run(&fx, &ctx2, &handle2, &grant2).await.unwrap(), outcome());
    assert_eq!(
        fx.spy.count(SandboxStep::Exec),
        calls,
        "replayed, not re-run"
    );
    assert!(fx.kinds().contains(&SandboxEventKind::SandboxExecCompleted));
}

// ---- backend errors ---------------------------------------------------------------------

#[tokio::test]
async fn backend_errors_keep_their_code_and_never_echo_the_request() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.spy.echo_error.store(true, Ordering::SeqCst);
    let err = run(&fx, &ctx, &handle, &grant).await.unwrap_err();
    assert_eq!(err.code(), "backend_error");
    let shown = format!("{err} {err:?} {}", serde_json::to_string(&err).unwrap());
    for hidden in ["s3cr3t", "tool", "token"] {
        assert!(!shown.contains(hidden), "{hidden} leaked in {shown}");
    }
    // A plain message passes through, shortened.
    fx.spy.echo_error.store(false, Ordering::SeqCst);
    fx.spy
        .script_exec(Err(SandboxError::Backend("x".repeat(1000))));
    let err = run(&fx, &ctx, &handle, &grant).await.unwrap_err();
    let SandboxError::Backend(message) = err else {
        panic!("expected a backend error");
    };
    assert_eq!(message.chars().count(), 256);
}

#[tokio::test]
async fn other_backend_errors_are_forwarded_unchanged() {
    for forwarded in [
        SandboxError::Unavailable { retriable: false },
        SandboxError::Busy {
            retry_after_ms: Some(5),
        },
        SandboxError::NotFound,
        SandboxError::Cancelled,
        SandboxError::PolicyDenied(PolicyReason::ProgramNotAllowed),
    ] {
        let fx = fx();
        let ctx = ctx();
        let (handle, grant) = ready(&fx, &ctx).await;
        fx.spy.script_exec(Err(forwarded.clone()));
        assert_eq!(
            run(&fx, &ctx, &handle, &grant).await.unwrap_err(),
            forwarded
        );
    }
}

#[tokio::test]
async fn a_backend_failure_on_create_is_forwarded_and_leaves_nothing_behind() {
    // A backend double that refuses `create`.
    struct Refusing;
    #[async_trait]
    impl SandboxBackend for Refusing {
        async fn create(
            &self,
            _: &SandboxContext,
            _: &IdempotencyKey,
            _: CreateRequest,
            _: &CancelToken,
        ) -> Result<RawCreated, SandboxError> {
            Err(SandboxError::Unavailable { retriable: true })
        }
        async fn exec(
            &self,
            _: &SandboxHandle,
            _: &IdempotencyKey,
            _: ExecRequest,
            _: &CancelToken,
        ) -> Result<ExecOutcome, SandboxError> {
            Err(SandboxError::NotFound)
        }
        async fn collect(
            &self,
            _: &SandboxHandle,
            _: &IdempotencyKey,
            _: CollectRequest,
            _: &CancelToken,
        ) -> Result<RawCollected, SandboxError> {
            Err(SandboxError::NotFound)
        }
        async fn destroy(&self, _: &SandboxHandle, _: &IdempotencyKey) -> Result<(), SandboxError> {
            Ok(())
        }
    }
    let guard = GovernedSandbox::new(
        Refusing,
        quota_policy(1, 100),
        Arc::new(ManualClock(AtomicU64::new(NOW))),
        Arc::new(InMemoryJournal::new()),
    );
    let ctx = ctx();
    let request = guard
        .request_approval(&ctx, &create_req(), &exec_req())
        .unwrap();
    let grant = ApprovalGrant {
        approval_id: "a".to_owned(),
        call_key: request.call_key,
        requested_by: "agent".to_owned(),
        approved_by: "alice".to_owned(),
    };
    for _ in 0..3 {
        // Three failures in a row would exhaust a ceiling of one if slots leaked.
        let err = guard
            .create(&ctx, create_req(), &exec_req(), Some(&grant), &never())
            .await
            .unwrap_err();
        assert_eq!(err, SandboxError::Unavailable { retriable: true });
    }
}

#[tokio::test]
async fn a_failed_exec_never_leaves_an_inconsistent_state() {
    // Unknown outcome: only the same call or destroy.
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.spy
        .script_exec(Err(SandboxError::Backend("boom".to_owned())));
    assert!(run(&fx, &ctx, &handle, &grant).await.is_err());
    let collect = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(code(collect), "invalid_state");
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();

    // A limit exceeded is a definite outcome: recorded, replayed, and collectable.
    let ctx2 = ctx_for("tenant-a", "node-2");
    let grant2 = grant_for(&fx, &ctx2, &create_req(), &exec_req());
    let handle2 = fx
        .guard
        .create(&ctx2, create_req(), &exec_req(), Some(&grant2), &never())
        .await
        .unwrap();
    let killed = SandboxError::LimitExceeded {
        kind: crate::error::LimitKind::WallTime,
        partial: Box::new(outcome()),
    };
    fx.spy.script_exec(Err(killed.clone()));
    assert_eq!(
        run(&fx, &ctx2, &handle2, &grant2).await.unwrap_err(),
        killed
    );
    let calls = fx.spy.count(SandboxStep::Exec);
    assert_eq!(
        run(&fx, &ctx2, &handle2, &grant2).await.unwrap_err(),
        killed
    );
    assert_eq!(fx.spy.count(SandboxStep::Exec), calls);
    fx.guard
        .collect(&ctx2, &handle2, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert!(fx.journal.inner.events().iter().any(|e| {
        e.kind == SandboxEventKind::SandboxExecFailed
            && e.code.as_deref() == Some("limit_exceeded.wall_time")
    }));

    // A violation poisons the sandbox: only destroy is accepted.
    let ctx3 = ctx_for("tenant-a", "node-3");
    let grant3 = grant_for(&fx, &ctx3, &create_req(), &exec_req());
    let handle3 = fx
        .guard
        .create(&ctx3, create_req(), &exec_req(), Some(&grant3), &never())
        .await
        .unwrap();
    fx.spy.script_exec(Err(SandboxError::IsolationViolation {
        signal: "escape-attempt".to_owned(),
    }));
    assert_eq!(
        run(&fx, &ctx3, &handle3, &grant3).await.unwrap_err().code(),
        "isolation_violation"
    );
    assert_eq!(
        run(&fx, &ctx3, &handle3, &grant3).await.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Poisoned
        }
    );
    let collect = fx
        .guard
        .collect(&ctx3, &handle3, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        collect.unwrap_err(),
        SandboxError::InvalidState {
            from: SandboxState::Poisoned
        }
    );
    fx.guard.destroy(&ctx3, &handle3).await.unwrap();
}

// ---- collect --------------------------------------------------------------------------

#[tokio::test]
async fn collected_output_is_redacted_and_its_digests_are_recomputed() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let mut raw = raw_collected(b"key=hunter2-value done", b"");
    raw.secrets.push(ResolvedSecret::new("hunter2-value"));
    let file_bytes = b"hunter2-value".to_vec();
    raw.files.push(RawFile {
        path: LogicalPath::new("out/a.txt").unwrap(),
        declared_sha256: sha256_hex(&file_bytes),
        bytes: file_bytes,
    });
    fx.spy.script_collect(Ok(raw));
    let collected = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(collected.stdout, b"key=[REDACTED] done");
    assert_eq!(collected.files[0].bytes, b"[REDACTED]");
    assert_eq!(collected.stdout_sha256, sha256_hex(&collected.stdout));
}

#[tokio::test]
async fn a_lying_or_oversized_delivery_is_refused_and_can_be_collected_again() {
    let fx = fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let mut lying = raw_collected(b"payload", b"");
    lying.declared_stdout_sha256 = sha256_hex(b"other");
    fx.spy.script_collect(Ok(lying));
    let big = raw_collected(&vec![0; Limits::default().output_bytes as usize + 1], b"");
    fx.spy.script_collect(Ok(big));
    for _ in 0..2 {
        let result = fx
            .guard
            .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
            .await;
        assert_eq!(code(result), "backend_error");
    }
    // The sandbox is still `Executed`: a good delivery goes through.
    fx.guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
}

// ---- destroy and with_sandbox ------------------------------------------------------------

#[tokio::test]
async fn destroy_is_idempotent() {
    let fx = fx();
    let ctx = ctx();
    let (handle, _) = ready(&fx, &ctx).await;
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 1);
    // A sandbox this guard does not know, of the same tenant, is released through the backend.
    let unknown = SandboxHandle::new(SandboxId::new("sb-unknown"), ctx.tenant_id.clone());
    fx.guard.destroy(&ctx, &unknown).await.unwrap();
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 2);
    let destroyed = fx
        .kinds()
        .iter()
        .filter(|k| **k == SandboxEventKind::SandboxDestroyed)
        .count();
    assert_eq!(destroyed, 1, "one event per key");
}

#[tokio::test]
async fn a_failed_destroy_keeps_the_sandbox_addressable() {
    let fx = fx_with(quota_policy(1, 100));
    let ctx = ctx();
    let (handle, _) = ready(&fx, &ctx).await;
    fx.spy.destroy_fails.store(true, Ordering::SeqCst);
    assert!(fx.guard.destroy(&ctx, &handle).await.is_err());
    // Still counted against the tenant, so the leak is visible.
    let refused = create_on(&fx, &ctx_for("tenant-a", "node-2")).await;
    assert_eq!(refused.unwrap_err().code(), "quota_exceeded");
    fx.spy.destroy_fails.store(false, Ordering::SeqCst);
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    create_on(&fx, &ctx_for("tenant-a", "node-2"))
        .await
        .unwrap();
}

#[tokio::test]
async fn with_sandbox_destroys_on_success() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let token = never();
    let planned = exec_req();
    let run_grant = grant.clone();
    let collected = fx
        .guard
        .with_sandbox(
            &ctx,
            create_req(),
            &planned,
            Some(&grant),
            &token,
            |session| {
                Box::pin(async move {
                    session
                        .exec(exec_req(), Some(&run_grant), RUN_STARTED)
                        .await?;
                    session.collect(collect_req(), RUN_STARTED).await
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(collected.stdout, b"hello");
    assert_eq!(fx.spy.steps().last(), Some(&SandboxStep::Destroy));
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 1);
}

#[tokio::test]
async fn with_sandbox_destroys_even_after_an_exec_failure() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let token = never();
    let planned = exec_req();
    fx.spy
        .script_exec(Err(SandboxError::Backend("boom".to_owned())));
    let run_grant = grant.clone();
    let result: Result<(), SandboxError> = fx
        .guard
        .with_sandbox(
            &ctx,
            create_req(),
            &planned,
            Some(&grant),
            &token,
            |session| {
                Box::pin(async move {
                    session
                        .exec(exec_req(), Some(&run_grant), RUN_STARTED)
                        .await?;
                    Ok(())
                })
            },
        )
        .await;
    assert_eq!(result.unwrap_err().code(), "backend_error");
    assert_eq!(
        fx.spy.steps(),
        vec![SandboxStep::Create, SandboxStep::Exec, SandboxStep::Destroy]
    );
}

#[tokio::test]
async fn with_sandbox_destroys_after_a_refused_exec_and_after_cancellation() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let planned = exec_req();
    // The node runs something the grant does not cover.
    let token = never();
    let result: Result<(), SandboxError> = fx
        .guard
        .with_sandbox(
            &ctx,
            create_req(),
            &planned,
            Some(&grant),
            &token,
            |session| {
                Box::pin(async move {
                    session.exec(other_exec_req(), None, RUN_STARTED).await?;
                    Ok(())
                })
            },
        )
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 1);

    // Cancellation requested while the node works: the exec is refused, the sandbox is
    // still destroyed (destroy ignores cancellation).
    let ctx2 = ctx_for("tenant-a", "node-2");
    let grant2 = grant_for(&fx, &ctx2, &create_req(), &exec_req());
    let flag = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&flag);
    let token = CancelToken::new(Arc::new(move || seen.load(Ordering::SeqCst)));
    let run_grant = grant2.clone();
    let result: Result<(), SandboxError> = fx
        .guard
        .with_sandbox(
            &ctx2,
            create_req(),
            &planned,
            Some(&grant2),
            &token,
            |session| {
                Box::pin(async move {
                    flag.store(true, Ordering::SeqCst);
                    session
                        .exec(exec_req(), Some(&run_grant), RUN_STARTED)
                        .await?;
                    Ok(())
                })
            },
        )
        .await;
    assert_eq!(result.unwrap_err(), SandboxError::Cancelled);
    assert_eq!(fx.spy.count(SandboxStep::Destroy), 2);
    assert_eq!(fx.spy.count(SandboxStep::Exec), 0);
}

#[tokio::test]
async fn with_sandbox_reports_a_failed_destroy_and_skips_everything_when_create_is_refused() {
    let fx = fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let planned = exec_req();
    let token = never();
    fx.spy.destroy_fails.store(true, Ordering::SeqCst);
    let result: Result<u8, SandboxError> = fx
        .guard
        .with_sandbox(&ctx, create_req(), &planned, Some(&grant), &token, |_| {
            Box::pin(async { Ok(7) })
        })
        .await;
    assert_eq!(
        result.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );

    // Without a grant nothing is created, so nothing is destroyed and the closure never runs.
    let fx = fx2();
    let result: Result<u8, SandboxError> = fx
        .guard
        .with_sandbox(&ctx, create_req(), &planned, None, &token, |_| {
            Box::pin(async { Ok(7) })
        })
        .await;
    assert_eq!(result.unwrap_err().code(), "approval_required");
    assert_eq!(fx.spy.total(), 0);
}

fn fx2() -> Fx {
    fx()
}

// ---- scrubbing --------------------------------------------------------------------------

#[test]
fn scrubbing_withholds_messages_that_echo_the_request() {
    let needles = vec![
        "--token=abcd1234".to_owned(),
        "line one\nsecretword".to_owned(),
    ];
    assert_eq!(scrub_message("failed", &needles), "failed");
    assert_eq!(
        scrub_message("bad --token=abcd1234", &needles),
        WITHHELD_MESSAGE
    );
    assert_eq!(scrub_message("saw abcd1234", &needles), WITHHELD_MESSAGE);
    assert_eq!(scrub_message("saw secretword", &needles), WITHHELD_MESSAGE);
    // Short words are not treated as an echo.
    assert_eq!(scrub_message("ok", &["ok".to_owned()]), "ok");
    assert_eq!(scrub_message(&"y".repeat(500), &[]).chars().count(), 256);
}

#[test]
fn bounds_error_fields_are_numbers_not_content() {
    let fx = fx();
    let mut huge = exec_req();
    huge.argv = vec!["a".repeat(40 * 1024)];
    let err = fx
        .guard
        .request_approval(&ctx(), &create_req(), &huge)
        .unwrap_err();
    assert_eq!(
        err,
        SandboxError::PolicyDenied(PolicyReason::RequestTooLarge {
            bound: crate::error::BoundKind::ArgvBytes,
            actual: 40 * 1024,
            max: 32 * 1024
        })
    );
}

#[tokio::test]
async fn debug_output_of_the_public_values_carries_no_request_content() {
    let fx = fx();
    let ctx = ctx();
    let request = fx
        .guard
        .request_approval(&ctx, &create_req(), &exec_req())
        .unwrap();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let refused = fx
        .guard
        .create(&ctx, create_req(), &exec_req(), None, &never())
        .await
        .unwrap_err();
    let shown = format!(
        "{request:?} {request:#?} {grant:?} {refused:?} {refused} {:?} {:?}",
        never(),
        fx.journal.inner.events()
    );
    for hidden in ["s3cr3t", "tool", "--token"] {
        assert!(!shown.contains(hidden), "{hidden} leaked in {shown}");
    }
}
