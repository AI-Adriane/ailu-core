//! The guard: the only way to reach a backend.
//!
//! [`GovernedSandbox`] validates every request against the deployment policy before any
//! backend call, builds the idempotency keys and the handles (a backend can build neither),
//! requires an approval bound to the exact call before anything is created or run, enforces
//! the lifecycle (one `exec` per sandbox and per node attempt), admits tenants fairly, detects
//! an isolation domain shared between tenants, honours cooperative cancellation and records
//! one event per transition in its own journal.
//!
//! Lifecycle: `Created -> Executing -> Executed -> Collected -> Destroyed`, plus `Poisoned`,
//! which accepts only `destroy`. `Executing` also means "the backend was asked and the outcome
//! is not known": only the same `exec` (same request, same key) or `destroy` is allowed from
//! there, so a failed or cancelled call never leaves the sandbox in an inconsistent state.
//!
//! The guard keeps its registry in memory. After a host restart the node is replayed: the
//! backend's idempotence answers `create` with the same sandbox and the registry is rebuilt by
//! the calls themselves. A sandbox whose host disappears before `destroy` is swept by the
//! backend when its `ttl` expires.

use crate::approval::{
    call_key, check_grant, create_fingerprint, create_subject, exec_fingerprint, exec_subject,
    subject, ApprovalGrant, ApprovalRequest,
};
use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::clock::SandboxClock;
use crate::collected::{derive, Collected};
use crate::digest::{sha256_hex, Canon};
use crate::error::{QuotaKind, SandboxError, SandboxState};
use crate::ids::{
    IdempotencyKey, IsolationDomain, SandboxContext, SandboxHandle, SandboxId, SandboxStep,
    TenantId,
};
use crate::journal::{SandboxEvent, SandboxEventKind, SandboxJournal};
use crate::outcome::ExecOutcome;
use crate::policy::SandboxPolicy;
use crate::request::{CollectRequest, CreateRequest, ExecRequest};
use crate::validate::{validate_collect, validate_context, validate_exec};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

/// Signal carried by the `IsolationViolation` the guard raises itself. Fixed text: it never
/// echoes an identifier supplied by a backend.
const SIGNAL_DOMAIN_SHARED: &str = "isolation_domain_shared";
const SIGNAL_ID_REUSED: &str = "sandbox_id_reused";

type ExecResult = Result<ExecOutcome, SandboxError>;

/// Future returned by the closure of [`GovernedSandbox::with_sandbox`].
pub type SessionFuture<'s, T> = Pin<Box<dyn Future<Output = Result<T, SandboxError>> + Send + 's>>;

struct ExecEntry {
    fingerprint: String,
    /// `None` while the outcome is not known (`Executing`).
    result: Option<ExecResult>,
}

struct Record {
    ctx: SandboxContext,
    handle: SandboxHandle,
    /// The isolation domain the backend declared for this sandbox.
    domain: IsolationDomain,
    state: SandboxState,
    /// The backend declared a domain already held by another tenant (or reused an id).
    violation: bool,
    create_fingerprint: String,
    create_subject: Value,
    /// Per-stream output cap of the sandbox, from its (validated) limits.
    output_cap: u64,
    exec: Option<ExecEntry>,
    collect: Option<(String, Collected)>,
}

struct CreateEntry {
    fingerprint: String,
    sandbox_id: SandboxId,
}

#[derive(Default)]
struct TenantUsage {
    /// Sandboxes created (or being created) and not yet destroyed.
    active: u32,
    /// Creation instants inside the rate window.
    created_at: VecDeque<u64>,
}

#[derive(Default)]
struct Inner {
    sandboxes: HashMap<String, Record>,
    creates: HashMap<String, CreateEntry>,
    /// First tenant that declared each isolation domain. Never forgotten, so a domain is
    /// never handed to another tenant, even after `destroy`.
    domains: HashMap<String, TenantId>,
    tenants: HashMap<String, TenantUsage>,
}

/// A refusal together with the sandbox it is about, if any.
struct Refusal {
    error: SandboxError,
    sandbox_id: Option<SandboxId>,
}

impl Refusal {
    fn new(error: SandboxError, sandbox_id: Option<SandboxId>) -> Self {
        Self { error, sandbox_id }
    }

    fn bare(error: SandboxError) -> Self {
        Self::new(error, None)
    }
}

enum ExecStart {
    /// The call already ran with this exact request: answer with the recorded result.
    Replay(ExecResult),
    Run,
}

enum CollectStart {
    Replay(Box<Collected>),
    Run { output_cap: u64 },
}

/// Errors worth a `sandbox_denied` event: refusals decided by the guard.
fn is_refusal(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::PolicyDenied(_)
            | SandboxError::ApprovalRequired { .. }
            | SandboxError::QuotaExceeded { .. }
            | SandboxError::InvalidPath { .. }
            | SandboxError::IdempotencyConflict
            | SandboxError::InvalidState { .. }
            | SandboxError::IsolationViolation { .. }
    )
}

const MAX_BACKEND_MESSAGE_CHARS: usize = 256;
const WITHHELD_MESSAGE: &str = "backend error (message withheld)";

/// Keep a backend's free-text message only when it cannot be echoing the request: it is cut
/// to a short length and replaced when it contains an argument, a word of the standard input
/// or any other string of the request (whole, or as a word of four characters or more).
/// Best effort: the backend is expected to redact; this is the second line.
fn scrub_message(message: &str, needles: &[String]) -> String {
    let shown: String = message.chars().take(MAX_BACKEND_MESSAGE_CHARS).collect();
    let echoes = |needle: &String| {
        (needle.chars().count() >= 3 && shown.contains(needle.as_str()))
            || needle
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word.chars().count() >= 4 && shown.contains(word))
    };
    if needles.iter().any(echoes) {
        WITHHELD_MESSAGE.to_owned()
    } else {
        shown
    }
}

/// Pass a backend error through unchanged (same variant, same code) except for its free
/// text, which is scrubbed.
fn scrub_error(error: SandboxError, needles: &[String]) -> SandboxError {
    match error {
        SandboxError::Backend(message) => SandboxError::Backend(scrub_message(&message, needles)),
        SandboxError::InvalidPath { reason } => SandboxError::InvalidPath {
            reason: scrub_message(&reason, needles),
        },
        SandboxError::IsolationViolation { signal } => SandboxError::IsolationViolation {
            signal: scrub_message(&signal, needles),
        },
        other => other,
    }
}

fn exec_needles(req: &ExecRequest) -> Vec<String> {
    let mut needles = req.argv.clone();
    if let Some(stdin) = &req.stdin {
        needles.push(String::from_utf8_lossy(stdin).into_owned());
    }
    needles
}

fn create_needles(req: &CreateRequest) -> Vec<String> {
    req.env
        .iter()
        .flat_map(|g| [g.name.clone(), g.secret_ref.0.clone()])
        .collect()
}

fn collect_fingerprint(req: &CollectRequest) -> String {
    let mut c = Canon::new("sandbox.collect.v1");
    c.num("patterns", req.patterns.len() as u64);
    for pattern in &req.patterns {
        c.str("pattern", pattern);
    }
    c.num("maxFiles", u64::from(req.max_files))
        .num("maxBytes", req.max_bytes);
    c.finish()
}

fn outcome_digest(result: &ExecResult) -> String {
    let bytes = match result {
        Ok(outcome) => serde_json::to_vec(outcome),
        Err(SandboxError::LimitExceeded { partial, .. }) => serde_json::to_vec(partial),
        Err(_) => return sha256_hex(b""),
    };
    sha256_hex(&bytes.unwrap_or_default())
}

fn unavailable() -> SandboxError {
    SandboxError::Unavailable { retriable: true }
}

/// The guard in front of one backend.
pub struct GovernedSandbox<B: SandboxBackend> {
    backend: B,
    policy: SandboxPolicy,
    clock: Arc<dyn SandboxClock>,
    journal: Arc<dyn SandboxJournal>,
    inner: Mutex<Inner>,
}

impl<B: SandboxBackend> GovernedSandbox<B> {
    pub fn new(
        backend: B,
        policy: SandboxPolicy,
        clock: Arc<dyn SandboxClock>,
        journal: Arc<dyn SandboxJournal>,
    ) -> Self {
        Self {
            backend,
            policy,
            clock,
            journal,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- approval ---------------------------------------------------------------------

    /// Describe the call a node is about to make, after validating both requests against the
    /// deployment policy. Nothing is created and the backend is not reached. The node files
    /// the result with the approval engine and suspends; once approved it passes the grant to
    /// [`GovernedSandbox::create`] and [`GovernedSandbox::exec`].
    pub fn request_approval(
        &self,
        ctx: &SandboxContext,
        create: &CreateRequest,
        exec: &ExecRequest,
    ) -> Result<ApprovalRequest, SandboxError> {
        validate_context(ctx)?;
        self.policy.check_create(create)?;
        validate_exec(exec, &self.policy.bounds)?;
        Ok(ApprovalRequest::for_call(ctx, create, exec))
    }

    // ---- journal ----------------------------------------------------------------------

    fn event(
        &self,
        ctx: &SandboxContext,
        kind: SandboxEventKind,
        key: &IdempotencyKey,
        sandbox_id: Option<&SandboxId>,
    ) -> SandboxEvent {
        SandboxEvent {
            kind,
            tenant_id: ctx.tenant_id.clone(),
            run_id: ctx.run_id.clone(),
            node_id: ctx.node_id.clone(),
            attempt: ctx.attempt,
            sandbox_id: sandbox_id.cloned(),
            key: key.as_str().to_owned(),
            request_digest: None,
            result_digest: None,
            isolation_domain: None,
            code: None,
            at_ms: self.clock.now_ms(),
        }
    }

    /// Record an event; a journal that cannot record fails the operation closed.
    async fn record(&self, event: SandboxEvent) -> Result<(), SandboxError> {
        self.journal
            .append(event)
            .await
            .map(|_| ())
            .map_err(|_| unavailable())
    }

    /// Journal a refusal (when it is one the guard decided) and hand the error back.
    async fn deny(
        &self,
        ctx: &SandboxContext,
        step: SandboxStep,
        refusal: Refusal,
    ) -> SandboxError {
        let Refusal { error, sandbox_id } = refusal;
        if is_refusal(&error) {
            let key = IdempotencyKey::new(ctx, step);
            let mut event = self.event(
                ctx,
                SandboxEventKind::SandboxDenied,
                &key,
                sandbox_id.as_ref(),
            );
            event.code = Some(error.full_code().to_owned());
            // The refusal stands even if the journal cannot record it.
            let _ = self.journal.append(event).await;
        }
        error
    }

    // ---- create -----------------------------------------------------------------------

    /// Reserve a sandbox for the call `planned` will make.
    ///
    /// Validates the request against the policy, requires a grant for the exact call
    /// (`create` plus `planned`), admits the tenant, then asks the backend. The same context
    /// with the same request returns the handle already obtained; with another request it is
    /// an `IdempotencyConflict`.
    pub async fn create(
        &self,
        ctx: &SandboxContext,
        req: CreateRequest,
        planned: &ExecRequest,
        grant: Option<&ApprovalGrant>,
        cancel: &CancelToken,
    ) -> Result<SandboxHandle, SandboxError> {
        let key = IdempotencyKey::new(ctx, SandboxStep::Create);
        let fingerprint = create_fingerprint(&req);
        match self.begin_create(ctx, &req, planned, grant, cancel, &key, &fingerprint) {
            Ok(CreateStart::Replay(handle)) => {
                self.emit_created(ctx, &key, &handle, &fingerprint).await?;
                return Ok(handle);
            }
            Ok(CreateStart::Run) => {}
            Err(refusal) => return Err(self.deny(ctx, SandboxStep::Create, refusal).await),
        }

        let needles = create_needles(&req);
        let output_cap = req.limits.output_bytes;
        let subject_part = create_subject(&req);
        let raw = match self.backend.create(ctx, &key, req, cancel).await {
            Ok(raw) => raw,
            Err(error) => {
                self.release_slot(&ctx.tenant_id);
                return Err(scrub_error(error, &needles));
            }
        };
        let landed = self.land_created(ctx, &key, &raw, &fingerprint, subject_part, output_cap);
        match landed {
            Landed::Created(handle) | Landed::Replay(handle) => {
                self.emit_created(ctx, &key, &handle, &fingerprint).await?;
                Ok(handle)
            }
            Landed::Violation { handle, signal } => {
                // The sandbox cannot be addressed by the caller: tear it down here. Its id
                // is new to the guard, so this cannot touch another tenant's sandbox.
                self.destroy_poisoned(ctx, &handle).await;
                let error = SandboxError::IsolationViolation {
                    signal: signal.to_owned(),
                };
                let refusal = Refusal::new(error, Some(handle.sandbox_id().clone()));
                Err(self.deny(ctx, SandboxStep::Create, refusal).await)
            }
            Landed::IdReused => {
                self.release_slot(&ctx.tenant_id);
                let error = SandboxError::IsolationViolation {
                    signal: SIGNAL_ID_REUSED.to_owned(),
                };
                Err(self
                    .deny(ctx, SandboxStep::Create, Refusal::bare(error))
                    .await)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_create(
        &self,
        ctx: &SandboxContext,
        req: &CreateRequest,
        planned: &ExecRequest,
        grant: Option<&ApprovalGrant>,
        cancel: &CancelToken,
        key: &IdempotencyKey,
        fingerprint: &str,
    ) -> Result<CreateStart, Refusal> {
        validate_context(ctx).map_err(Refusal::bare)?;
        self.policy.check_create(req).map_err(Refusal::bare)?;
        validate_exec(planned, &self.policy.bounds).map_err(Refusal::bare)?;

        let mut inner = self.inner();
        if let Some(entry) = inner.creates.get(key.as_str()) {
            let record = inner.sandboxes.get(entry.sandbox_id.as_str());
            if entry.fingerprint != fingerprint {
                return Err(Refusal::new(
                    SandboxError::IdempotencyConflict,
                    Some(entry.sandbox_id.clone()),
                ));
            }
            return match record {
                Some(r) if r.violation => Err(Refusal::new(
                    SandboxError::IsolationViolation {
                        signal: SIGNAL_DOMAIN_SHARED.to_owned(),
                    },
                    Some(r.handle.sandbox_id().clone()),
                )),
                Some(r) => Ok(CreateStart::Replay(r.handle.clone())),
                None => Err(Refusal::bare(SandboxError::NotFound)),
            };
        }

        let planned_fp = exec_fingerprint(planned);
        let expected = call_key(ctx, fingerprint, &planned_fp);
        check_grant(grant, &expected, || {
            subject(ctx, &expected, &create_subject(req), &exec_subject(planned))
        })
        .map_err(Refusal::bare)?;
        if cancel.is_cancelled() {
            return Err(Refusal::bare(SandboxError::Cancelled));
        }
        self.reserve_slot(&mut inner, ctx).map_err(Refusal::bare)?;
        Ok(CreateStart::Run)
    }

    /// Admission control, per tenant, answered at once. Counts the slot as taken.
    fn reserve_slot(&self, inner: &mut Inner, ctx: &SandboxContext) -> Result<(), SandboxError> {
        let quota = &self.policy.tenant_quota;
        let now = self.clock.now_ms();
        let usage = inner
            .tenants
            .entry(ctx.tenant_id.as_str().to_owned())
            .or_default();
        while usage
            .created_at
            .front()
            .is_some_and(|at| now.saturating_sub(*at) >= quota.window_ms)
        {
            usage.created_at.pop_front();
        }
        if usage.active >= quota.max_concurrent {
            return Err(SandboxError::QuotaExceeded {
                kind: QuotaKind::TenantConcurrency,
            });
        }
        if usage.created_at.len() as u64 >= u64::from(quota.max_creations) {
            return Err(SandboxError::QuotaExceeded {
                kind: QuotaKind::CreationRate,
            });
        }
        usage.active += 1;
        usage.created_at.push_back(now);
        Ok(())
    }

    fn release_slot(&self, tenant: &TenantId) {
        let mut inner = self.inner();
        if let Some(usage) = inner.tenants.get_mut(tenant.as_str()) {
            usage.active = usage.active.saturating_sub(1);
        }
    }

    fn land_created(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        raw: &crate::ids::RawCreated,
        fingerprint: &str,
        subject_part: Value,
        output_cap: u64,
    ) -> Landed {
        let mut inner = self.inner();
        // Another call with the same key landed while this one waited for the backend.
        if let Some(entry) = inner.creates.get(key.as_str()) {
            let handle = inner
                .sandboxes
                .get(entry.sandbox_id.as_str())
                .map(|r| r.handle.clone());
            if let Some(handle) = handle {
                if let Some(usage) = inner.tenants.get_mut(ctx.tenant_id.as_str()) {
                    usage.active = usage.active.saturating_sub(1);
                }
                return Landed::Replay(handle);
            }
        }
        if inner.sandboxes.contains_key(raw.sandbox_id.as_str()) {
            return Landed::IdReused;
        }
        let handle = SandboxHandle::from_created(raw, &ctx.tenant_id);
        let domain_owner = inner.domains.get(raw.isolation_domain.as_str()).cloned();
        let violation = domain_owner.is_some_and(|owner| owner != ctx.tenant_id);
        if !violation {
            inner.domains.insert(
                raw.isolation_domain.as_str().to_owned(),
                ctx.tenant_id.clone(),
            );
        }
        inner.creates.insert(
            key.as_str().to_owned(),
            CreateEntry {
                fingerprint: fingerprint.to_owned(),
                sandbox_id: raw.sandbox_id.clone(),
            },
        );
        inner.sandboxes.insert(
            raw.sandbox_id.as_str().to_owned(),
            Record {
                ctx: ctx.clone(),
                handle: handle.clone(),
                domain: raw.isolation_domain.clone(),
                state: if violation {
                    SandboxState::Poisoned
                } else {
                    SandboxState::Created
                },
                violation,
                create_fingerprint: fingerprint.to_owned(),
                create_subject: subject_part,
                output_cap,
                exec: None,
                collect: None,
            },
        );
        if violation {
            Landed::Violation {
                handle,
                signal: SIGNAL_DOMAIN_SHARED,
            }
        } else {
            Landed::Created(handle)
        }
    }

    async fn emit_created(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        handle: &SandboxHandle,
        fingerprint: &str,
    ) -> Result<(), SandboxError> {
        let domain = self.domain_of(handle);
        let mut event = self.event(
            ctx,
            SandboxEventKind::SandboxCreated,
            key,
            Some(handle.sandbox_id()),
        );
        event.request_digest = Some(fingerprint.to_owned());
        event.isolation_domain = domain;
        self.record(event).await
    }

    fn domain_of(&self, handle: &SandboxHandle) -> Option<IsolationDomain> {
        let inner = self.inner();
        inner
            .sandboxes
            .get(handle.sandbox_id().as_str())
            .map(|record| record.domain.clone())
    }

    async fn destroy_poisoned(&self, ctx: &SandboxContext, handle: &SandboxHandle) {
        let key = IdempotencyKey::new(ctx, SandboxStep::Destroy);
        if self.backend.destroy(handle, &key).await.is_ok() {
            self.mark_destroyed(handle);
        }
    }

    fn mark_destroyed(&self, handle: &SandboxHandle) {
        let mut inner = self.inner();
        let mut release = None;
        if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            if record.state != SandboxState::Destroyed {
                record.state = SandboxState::Destroyed;
                release = Some(record.ctx.tenant_id.clone());
            }
        }
        if let Some(tenant) = release {
            if let Some(usage) = inner.tenants.get_mut(tenant.as_str()) {
                usage.active = usage.active.saturating_sub(1);
            }
        }
    }
}

enum CreateStart {
    Replay(SandboxHandle),
    Run,
}

enum Landed {
    Created(SandboxHandle),
    Replay(SandboxHandle),
    Violation {
        handle: SandboxHandle,
        signal: &'static str,
    },
    IdReused,
}

// ---- exec ---------------------------------------------------------------------------

impl<B: SandboxBackend> GovernedSandbox<B> {
    /// Run the one program the sandbox may run.
    ///
    /// Order of checks, all before the backend: the context and the handle (another tenant's
    /// handle is `NotFound`), the request bounds, the age of the attempt
    /// (`run_started_at_ms` is the instant the run started, a datum of the host), the state,
    /// the approval for this exact call, and cancellation.
    ///
    /// The same request again returns the recorded result without calling the backend. A
    /// different request after the program ran is `InvalidState`: one `exec` per sandbox.
    pub async fn exec(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: ExecRequest,
        grant: Option<&ApprovalGrant>,
        run_started_at_ms: u64,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        let key = IdempotencyKey::new(ctx, SandboxStep::Exec);
        let fingerprint = exec_fingerprint(&req);
        let start = self.begin_exec(
            ctx,
            handle,
            &req,
            &fingerprint,
            grant,
            run_started_at_ms,
            cancel,
        );
        match start {
            Err(refusal) => return Err(self.deny(ctx, SandboxStep::Exec, refusal).await),
            Ok(ExecStart::Replay(result)) => {
                let ended = self
                    .emit_exec_end(ctx, &key, handle, &fingerprint, &result)
                    .await;
                return match (result, ended) {
                    (Ok(_), Err(error)) => Err(error),
                    (result, _) => result,
                };
            }
            Ok(ExecStart::Run) => {}
        }

        let mut started = self.event(
            ctx,
            SandboxEventKind::SandboxExecStarted,
            &key,
            Some(handle.sandbox_id()),
        );
        started.request_digest = Some(fingerprint.clone());
        self.record(started).await?;

        let needles = exec_needles(&req);
        let result = self
            .backend
            .exec(handle, &key, req, cancel)
            .await
            .map_err(|error| scrub_error(error, &needles));
        self.settle_exec(handle, &result);
        let ended = self
            .emit_exec_end(ctx, &key, handle, &fingerprint, &result)
            .await;
        match (result, ended) {
            (Ok(_), Err(error)) => Err(error),
            (result, _) => result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_exec(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: &ExecRequest,
        fingerprint: &str,
        grant: Option<&ApprovalGrant>,
        run_started_at_ms: u64,
        cancel: &CancelToken,
    ) -> Result<ExecStart, Refusal> {
        validate_context(ctx).map_err(Refusal::bare)?;
        let mut inner = self.inner();
        let record = Self::addressed(&mut inner, ctx, handle).map_err(Refusal::bare)?;
        let refuse = |error| Refusal::new(error, Some(record.handle.sandbox_id().clone()));
        validate_exec(req, &self.policy.bounds).map_err(refuse)?;
        self.policy
            .check_attempt_age(self.clock.now_ms(), run_started_at_ms)
            .map_err(refuse)?;

        let same_request =
            record.exec.as_ref().map(|e| e.fingerprint.as_str()) == Some(fingerprint);
        match record.state {
            SandboxState::Created => {}
            SandboxState::Executing => {
                if !same_request {
                    return Err(refuse(SandboxError::IdempotencyConflict));
                }
            }
            SandboxState::Executed | SandboxState::Collected => {
                let recorded = record.exec.as_ref().and_then(|e| e.result.clone());
                return match recorded {
                    Some(result) if same_request => Ok(ExecStart::Replay(result)),
                    _ => Err(refuse(SandboxError::InvalidState { from: record.state })),
                };
            }
            SandboxState::Poisoned | SandboxState::Destroyed => {
                return Err(refuse(SandboxError::InvalidState { from: record.state }));
            }
        }

        let expected = call_key(ctx, &record.create_fingerprint, fingerprint);
        check_grant(grant, &expected, || {
            subject(ctx, &expected, &record.create_subject, &exec_subject(req))
        })
        .map_err(refuse)?;
        if cancel.is_cancelled() {
            return Err(refuse(SandboxError::Cancelled));
        }
        record.state = SandboxState::Executing;
        record.exec = Some(ExecEntry {
            fingerprint: fingerprint.to_owned(),
            result: None,
        });
        Ok(ExecStart::Run)
    }

    /// The record `handle` addresses for `ctx`, or `NotFound`: another tenant's handle, another
    /// context's handle and an unknown sandbox all look the same.
    fn addressed<'i>(
        inner: &'i mut Inner,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<&'i mut Record, SandboxError> {
        if handle.tenant_id() != &ctx.tenant_id {
            return Err(SandboxError::NotFound);
        }
        match inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            Some(record) if record.ctx == *ctx && record.handle == *handle => Ok(record),
            _ => Err(SandboxError::NotFound),
        }
    }

    fn settle_exec(&self, handle: &SandboxHandle, result: &ExecResult) {
        let mut inner = self.inner();
        let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) else {
            return;
        };
        match result {
            Ok(_) | Err(SandboxError::LimitExceeded { .. }) => {
                record.state = SandboxState::Executed;
                if let Some(entry) = record.exec.as_mut() {
                    entry.result = Some(result.clone());
                }
            }
            Err(SandboxError::IsolationViolation { .. }) => {
                record.state = SandboxState::Poisoned;
            }
            // Cancelled, busy, unavailable, backend failure…: the outcome is unknown, so the
            // sandbox stays `Executing`: only the same call or `destroy` can follow.
            Err(_) => {}
        }
    }

    async fn emit_exec_end(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        handle: &SandboxHandle,
        fingerprint: &str,
        result: &ExecResult,
    ) -> Result<(), SandboxError> {
        let kind = if result.is_ok() {
            SandboxEventKind::SandboxExecCompleted
        } else {
            SandboxEventKind::SandboxExecFailed
        };
        let mut event = self.event(ctx, kind, key, Some(handle.sandbox_id()));
        event.request_digest = Some(fingerprint.to_owned());
        match result {
            Ok(_) => event.result_digest = Some(outcome_digest(result)),
            Err(error) => {
                event.code = Some(error.full_code().to_owned());
                if matches!(error, SandboxError::LimitExceeded { .. }) {
                    event.result_digest = Some(outcome_digest(result));
                }
            }
        }
        self.record(event).await
    }
}

// ---- collect ------------------------------------------------------------------------

impl<B: SandboxBackend> GovernedSandbox<B> {
    /// Retrieve the outputs of the program that ran. Only valid after `exec` (also after a
    /// limit was exceeded, so that what was produced can still be read). The bytes are
    /// redacted by the secret values the backend resolved, and the digests are recomputed by
    /// the guard over what it received.
    pub async fn collect(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: CollectRequest,
        run_started_at_ms: u64,
        cancel: &CancelToken,
    ) -> Result<Collected, SandboxError> {
        let key = IdempotencyKey::new(ctx, SandboxStep::Collect);
        let fingerprint = collect_fingerprint(&req);
        let start = self.begin_collect(ctx, handle, &req, &fingerprint, run_started_at_ms, cancel);
        let output_cap = match start {
            Err(refusal) => return Err(self.deny(ctx, SandboxStep::Collect, refusal).await),
            Ok(CollectStart::Replay(collected)) => {
                self.emit_collected(ctx, &key, handle, &fingerprint, &collected)
                    .await?;
                return Ok(*collected);
            }
            Ok(CollectStart::Run { output_cap }) => output_cap,
        };

        let needles = req.patterns.clone();
        let raw = match self
            .backend
            .collect(handle, &key, req.clone(), cancel)
            .await
        {
            Ok(raw) => raw,
            Err(error) => {
                if matches!(error, SandboxError::IsolationViolation { .. }) {
                    self.poison(handle);
                    let refusal = Refusal::new(
                        scrub_error(error, &needles),
                        Some(handle.sandbox_id().clone()),
                    );
                    return Err(self.deny(ctx, SandboxStep::Collect, refusal).await);
                }
                return Err(scrub_error(error, &needles));
            }
        };
        let collected = derive(raw, output_cap, &req)?;
        self.settle_collect(handle, &fingerprint, &collected);
        self.emit_collected(ctx, &key, handle, &fingerprint, &collected)
            .await?;
        Ok(collected)
    }

    fn begin_collect(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: &CollectRequest,
        fingerprint: &str,
        run_started_at_ms: u64,
        cancel: &CancelToken,
    ) -> Result<CollectStart, Refusal> {
        validate_context(ctx).map_err(Refusal::bare)?;
        let mut inner = self.inner();
        let record = Self::addressed(&mut inner, ctx, handle).map_err(Refusal::bare)?;
        let refuse = |error| Refusal::new(error, Some(record.handle.sandbox_id().clone()));
        validate_collect(req, &self.policy.bounds).map_err(refuse)?;
        self.policy
            .check_attempt_age(self.clock.now_ms(), run_started_at_ms)
            .map_err(refuse)?;
        match record.state {
            SandboxState::Executed => {}
            SandboxState::Collected => {
                return match &record.collect {
                    Some((recorded, collected)) if recorded == fingerprint => {
                        Ok(CollectStart::Replay(Box::new(collected.clone())))
                    }
                    _ => Err(refuse(SandboxError::InvalidState { from: record.state })),
                };
            }
            SandboxState::Created
            | SandboxState::Executing
            | SandboxState::Poisoned
            | SandboxState::Destroyed => {
                return Err(refuse(SandboxError::InvalidState { from: record.state }));
            }
        }
        if cancel.is_cancelled() {
            return Err(refuse(SandboxError::Cancelled));
        }
        Ok(CollectStart::Run {
            output_cap: record.output_cap,
        })
    }

    fn settle_collect(&self, handle: &SandboxHandle, fingerprint: &str, collected: &Collected) {
        let mut inner = self.inner();
        if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            record.state = SandboxState::Collected;
            record.collect = Some((fingerprint.to_owned(), collected.clone()));
        }
    }

    fn poison(&self, handle: &SandboxHandle) {
        let mut inner = self.inner();
        if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            if record.state != SandboxState::Destroyed {
                record.state = SandboxState::Poisoned;
            }
        }
    }

    async fn emit_collected(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        handle: &SandboxHandle,
        fingerprint: &str,
        collected: &Collected,
    ) -> Result<(), SandboxError> {
        let mut event = self.event(
            ctx,
            SandboxEventKind::SandboxCollected,
            key,
            Some(handle.sandbox_id()),
        );
        event.request_digest = Some(fingerprint.to_owned());
        event.result_digest = Some(collected.outputs_digest.clone());
        self.record(event).await
    }
}

// ---- destroy and the node-scoped session ----------------------------------------------

impl<B: SandboxBackend> GovernedSandbox<B> {
    /// Release the sandbox. Idempotent: an already destroyed sandbox, or one the guard does
    /// not know for this tenant, succeeds. Not cancellable. Another tenant's handle is
    /// `NotFound`.
    pub async fn destroy(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<(), SandboxError> {
        validate_context(ctx)?;
        if handle.tenant_id() != &ctx.tenant_id {
            return Err(SandboxError::NotFound);
        }
        let key = IdempotencyKey::new(ctx, SandboxStep::Destroy);
        let already = {
            let inner = self.inner();
            match inner.sandboxes.get(handle.sandbox_id().as_str()) {
                Some(record) if record.ctx != *ctx || record.handle != *handle => {
                    return Err(SandboxError::NotFound);
                }
                Some(record) => record.state == SandboxState::Destroyed,
                None => false,
            }
        };
        if !already {
            self.backend
                .destroy(handle, &key)
                .await
                .map_err(|error| scrub_error(error, &[]))?;
            self.mark_destroyed(handle);
        }
        let event = self.event(
            ctx,
            SandboxEventKind::SandboxDestroyed,
            &key,
            Some(handle.sandbox_id()),
        );
        self.record(event).await
    }

    /// Run `f` with a sandbox that lives at most as long as this call. The sandbox is created
    /// (after the approval check), handed to `f` as a [`SandboxSession`], and destroyed when
    /// `f` ends, whether it succeeded, failed or was cancelled. If `f` succeeded but the
    /// destruction failed, the destruction error is returned: a leaked sandbox is not
    /// silent. If the returned future is dropped before it completes, destruction is left to
    /// the backend's `ttl` sweep.
    pub async fn with_sandbox<'a, T, F>(
        &'a self,
        ctx: &'a SandboxContext,
        create: CreateRequest,
        planned: &ExecRequest,
        grant: Option<&ApprovalGrant>,
        cancel: &'a CancelToken,
        f: F,
    ) -> Result<T, SandboxError>
    where
        F: for<'s> FnOnce(&'s SandboxSession<'a, B>) -> SessionFuture<'s, T>,
    {
        let handle = self.create(ctx, create, planned, grant, cancel).await?;
        let session = SandboxSession {
            guard: self,
            ctx,
            handle,
            cancel,
        };
        let outcome = f(&session).await;
        let destroyed = self.destroy(ctx, &session.handle).await;
        match (outcome, destroyed) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

/// A sandbox bound to its context and cancellation token, handed to the closure of
/// [`GovernedSandbox::with_sandbox`].
pub struct SandboxSession<'a, B: SandboxBackend> {
    guard: &'a GovernedSandbox<B>,
    ctx: &'a SandboxContext,
    handle: SandboxHandle,
    cancel: &'a CancelToken,
}

impl<B: SandboxBackend> SandboxSession<'_, B> {
    pub fn handle(&self) -> &SandboxHandle {
        &self.handle
    }

    pub async fn exec(
        &self,
        req: ExecRequest,
        grant: Option<&ApprovalGrant>,
        run_started_at_ms: u64,
    ) -> Result<ExecOutcome, SandboxError> {
        self.guard
            .exec(
                self.ctx,
                &self.handle,
                req,
                grant,
                run_started_at_ms,
                self.cancel,
            )
            .await
    }

    pub async fn collect(
        &self,
        req: CollectRequest,
        run_started_at_ms: u64,
    ) -> Result<Collected, SandboxError> {
        self.guard
            .collect(self.ctx, &self.handle, req, run_started_at_ms, self.cancel)
            .await
    }
}

#[cfg(test)]
mod tests;
