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
//! Concurrency and leaks. `Destroyed` is terminal: a late `exec` or `collect` that returns
//! after a `destroy` never brings the sandbox back, and the tenant's slot is given back once,
//! at the transition to `Destroyed`. A slot reserved by a `create` that is dropped, panics or
//! fails is given back by a drop guard; a sandbox that is never destroyed (its owner was
//! dropped or panicked) gives its slot back when the injected clock passes its creation time
//! plus its `ttl`, the same instant the backend sweeps it. Only one `exec` of a given key
//! reaches the backend at a time; a concurrent one is answered `Busy`. After an error that
//! says the call cannot be retried, the same call is answered with that error again instead
//! of reaching the backend. Clock reads and cancellation checks happen before the registry
//! lock is taken, and a poisoned lock fails closed.
//!
//! The registry is bounded, and a tenant can only fill its own share. Finished sandboxes are
//! compacted to a minimal trace (keys, request fingerprints, digests, times) that no longer
//! counts as a live record; live records are capped per tenant and overall, and a tenant's
//! oldest traces go first when it has too many. Isolation domains are capped per tenant and in
//! the table: a domain stays in the table, so is never given to another tenant, until it is
//! retired and is the tenant's (or, when the table is full, the table's) least recently used.
//! Each `create` walks the registry once (O(n) in a size the policy bounds). The clock is
//! trusted only up to a point: while sandboxes are live, a step forward of more than the
//! largest `ttl` plus the skew allowance is refused (`Unavailable`, retriable) and time is not
//! advanced.
//!
//! Backend text is never relayed: a backend error keeps its code and loses its message, the
//! variants only the guard may raise (policy, approval, quota, idempotency, state, not found)
//! become a generic `Backend` error when a backend returns them, and the partial outcome of a
//! killed program is reduced to bounded, plain values.
//!
//! A `create` repeated with the same key and request returns the handle already obtained
//! without asking for the approval again: the handle opens nothing, and every `exec` still
//! needs its own verified approval. If the backend hands back a sandbox identifier the guard
//! already knows, the guard refuses and leaves the resource to the backend's `ttl` sweep: it
//! cannot tell whose it is, so it must not destroy it. A `NotFound` is not journaled per
//! occurrence (that would let anyone fill the journal by guessing handles); the guard only
//! counts them ([`GovernedSandbox::not_found_count`]).
//!
//! Proof. With a [`ProofSigner`] (opt-in, [`GovernedSandbox::with_proof_signer`]) `collect`
//! builds the execution proof from the digests the guard recomputed, has it signed, and only
//! then records the result and returns it. A signer that is unavailable or refuses fails the
//! call closed and leaves the sandbox collectable: the program has already run, the backend
//! keeps its result, and the same call made again signs it. A replayed `collect` is signed
//! again too, so a result is never served without its signature. The approval that let `exec`
//! through is remembered at that call (`collect` has no claim) for the proof to state.
//!
//! The guard keeps its registry in memory. After a host restart the node is replayed: the
//! backend's idempotence answers `create` with the same sandbox and the registry is rebuilt by
//! the calls themselves. A sandbox whose host disappears before `destroy` is swept by the
//! backend when its `ttl` expires.

use crate::approval::{
    call_key, check_create_reviewable, check_exec_reviewable, check_grant, create_counts,
    create_fingerprint, exec_counts, exec_fingerprint, minimal_subject, verify_claim,
    ApprovalClaim, ApprovalRequest, GrantVerifier, Verification,
};
use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::clock::SandboxClock;
use crate::collected::{derive, Collected, ProvedCollected};
use crate::digest::{sha256_hex, Canon};
use crate::error::{PolicyReason, QuotaKind, SandboxError, SandboxState};
use crate::ids::{
    IdempotencyKey, IsolationDomain, RawCreated, SandboxContext, SandboxHandle, SandboxId,
    SandboxStep, TenantId,
};
use crate::journal::{SandboxEvent, SandboxEventKind, SandboxJournal};
use crate::limits::Limits;
use crate::outcome::{DeclaredExit, ExecOutcome};
use crate::policy::{AttemptWindow, PolicyConfigError, SandboxPolicy};
use crate::proof::{
    payload_hash, ExecutionProof, ProofApproval, ProofSigner, SignedExecutionProof, SignerError,
    PROOF_ALGORITHM, PROOF_DOMAIN, PROOF_KIND,
};
use crate::request::{CollectRequest, CreateRequest, ExecRequest};
use crate::validate::{validate_collect, validate_context, validate_exec};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Signals of the `IsolationViolation` the guard raises, or reports for a backend. Fixed
/// text: it never echoes anything supplied by a backend.
const SIGNAL_DOMAIN_SHARED: &str = "isolation_domain_shared";
const SIGNAL_ID_REUSED: &str = "sandbox_id_reused";
const SIGNAL_BACKEND: &str = "reported_by_backend";
const GENERIC_BACKEND_MESSAGE: &str = "backend error";
const REJECTED_PATH_REASON: &str = "path rejected by backend";
/// What a concurrent caller is told to wait before asking again.
const BUSY_RETRY_MS: u64 = 250;
const MAX_SIGNAL_CHARS: usize = 32;
const MAX_TRUNCATION_MARKERS: usize = 2;

type ExecResult = Result<ExecOutcome, SandboxError>;

/// Future returned by the closure of [`GovernedSandbox::with_sandbox`].
pub type SessionFuture<'s, T> = Pin<Box<dyn Future<Output = Result<T, SandboxError>> + Send + 's>>;

struct ExecEntry {
    fingerprint: String,
    /// Digest of the definite outcome, kept when the record is compacted.
    result_digest: Option<String>,
    /// The definite outcome; `None` while it is not known (`Executing`).
    result: Option<ExecResult>,
    /// An error after which the same call must not reach the backend again.
    terminal: Option<SandboxError>,
    /// The approval that let this `exec` through, kept because `collect` carries no claim.
    grant: Option<GrantTrace>,
    /// When the guard sent the program to the backend (its clock), for the proof.
    started_at: u64,
    /// When the backend answered with a definite outcome (its clock), for the proof.
    finished_at: Option<u64>,
}

/// The approval the verifier established for an `exec`: what the proof carries of it.
#[derive(Clone)]
struct GrantTrace {
    call_key: String,
    approved_by: String,
    approval_payload_hash: String,
}

/// What the proof takes from the `create` request. Kept only when a signer is configured.
#[derive(Clone)]
struct ProofBasis {
    profile_digest: String,
    limits: Limits,
    inputs_digest: String,
}

/// Everything the guard knows about a sandbox that the proof of its `collect` is built from.
struct ProofParts {
    sandbox_id: SandboxId,
    domain: IsolationDomain,
    create_fingerprint: String,
    exec_fingerprint: String,
    outcome: ExecOutcome,
    grant: GrantTrace,
    started_at: u64,
    finished_at: u64,
    basis: ProofBasis,
    violation: bool,
}

struct Record {
    ctx: SandboxContext,
    handle: SandboxHandle,
    /// The isolation domain the backend declared for this sandbox.
    domain: IsolationDomain,
    state: SandboxState,
    /// The backend declared a domain already held by another tenant.
    violation: bool,
    create_key: String,
    create_fingerprint: String,
    create_counts: Value,
    /// Digest of the approval that authorized the creation, kept so that a replayed creation
    /// event links to the approval that actually covered the call.
    create_payload_hash: Option<String>,
    /// Per-stream output cap of the sandbox, from its (validated) limits.
    output_cap: u64,
    /// How many secret grants the sandbox was created with: the backend must hand back at
    /// least that many resolved values at `collect`, or the delivery is refused.
    secret_grants: usize,
    /// Creation time plus `ttl`: the backend sweeps the sandbox then, and so does the guard.
    expires_at: u64,
    /// When the record is dropped from the registry once it is over.
    evict_at: u64,
    /// The sandbox still counts against its tenant's concurrency ceiling.
    slot_held: bool,
    /// How many `destroy` calls are at the backend: nothing else may start while any is.
    /// A counter, not a flag, so one destroy failing cannot reopen the sandbox under another.
    closing: u32,
    /// Over: the record keeps only a minimal trace (keys, fingerprints, digests, times).
    finished: bool,
    /// An `exec` is at the backend right now.
    in_flight: bool,
    exec: Option<ExecEntry>,
    collect: Option<(String, Collected)>,
    /// What `create` asked for, as the proof will state it (only with a signer).
    basis: Option<ProofBasis>,
    /// The proof built for the recorded `collect`; a replayed `collect` has it signed again.
    proof: Option<ExecutionProof>,
}

impl Record {
    /// The state operations see: a sandbox being destroyed, past its `ttl`, or already over
    /// (compacted) is destroyed. A finished record stays terminal even when the guard's time
    /// is later set back.
    fn view(&self, now: u64) -> SandboxState {
        if self.state != SandboxState::Destroyed
            && (self.finished || self.closing > 0 || now >= self.expires_at)
        {
            SandboxState::Destroyed
        } else {
            self.state
        }
    }

    /// Reduce a finished record to a minimal trace: the keys, the request fingerprints, the
    /// digests and the times stay; bytes, subjects and results go.
    fn compact(&mut self) {
        self.finished = true;
        self.strip();
        self.create_counts = Value::Null;
        if let Some(entry) = self.exec.as_mut() {
            entry.result = None;
        }
    }

    /// Drop the collected bytes; keep the digests.
    fn strip(&mut self) {
        if let Some((_, collected)) = self.collect.as_mut() {
            collected.stdout = Vec::new();
            collected.stderr = Vec::new();
            for file in &mut collected.files {
                file.bytes = Vec::new();
            }
        }
    }
}

struct DomainEntry {
    tenant: TenantId,
    /// Order of last use (a counter, not a time).
    last_used: u64,
}

struct CreateEntry {
    fingerprint: String,
    sandbox_id: SandboxId,
}

#[derive(Default)]
struct TenantUsage {
    /// Sandboxes created (or being created) and not yet destroyed or expired.
    active: u32,
    /// Creation instants inside the rate window (non-decreasing).
    created_at: VecDeque<u64>,
    /// Creates at the backend that hold registry capacity but have not landed a record yet.
    pending: usize,
}

#[derive(Default)]
struct Inner {
    /// Creates at the backend that hold registry capacity but have not landed a record yet,
    /// over all tenants. A record replaces its reservation when it lands.
    pending_creates: usize,
    sandboxes: HashMap<String, Record>,
    creates: HashMap<String, CreateEntry>,
    /// Owner of each isolation domain in the table. While a domain is in the table it is never
    /// given to another tenant, even after `destroy`; retired domains leave it least recently
    /// used first, per tenant, when a tenant is at its bound.
    domains: HashMap<String, DomainEntry>,
    domain_seq: u64,
    tenants: HashMap<String, TenantUsage>,
    /// Highest instant seen: a clock that steps back does not move time backwards here.
    high_water: u64,
    /// A forward step too large to be taken on one reading, waiting for the next reading to
    /// confirm it.
    suspect: Option<u64>,
}

impl Inner {
    /// Advance the highest instant seen, and bring it back when the clock was wrong.
    ///
    /// A small step back does not move time back (a clock that slews). A step forward of at
    /// most `max_jump` (a ttl plus the skew allowance) is taken at once. A larger one is not
    /// taken on a single reading while sandboxes are live, because one aberrant reading would
    /// expire every live sandbox of every tenant: the call that carries it is refused (fail
    /// closed, retriable) and the reading is remembered; the next reading that does not go back
    /// below it confirms the step (a long idle period, or a clock that really moved), and a
    /// reading back within the allowance forgets it. A first reading, and a step with nothing
    /// live, are taken as they are.
    ///
    /// A step back of more than `max_jump` means an earlier reading was wrong (or the clock was
    /// corrected): the guard's time is set back to the reading, and the end times of the
    /// records that were made under the wrong time are brought back to what the longest ttl and
    /// retention allow from now, so that nothing keeps its slot or its memory until a date in
    /// the future.
    fn tick(&mut self, now: u64, policy: &SandboxPolicy) -> Result<u64, SandboxError> {
        let max_jump = policy
            .bounds
            .max_ttl_ms
            .saturating_add(policy.max_clock_skew_ms);
        if self.high_water != 0 && now.saturating_add(max_jump) < self.high_water {
            self.resync(now, policy);
            return Ok(self.high_water);
        }
        let big = now > self.high_water.saturating_add(max_jump)
            && self.high_water != 0
            && self.sandboxes.values().any(|r| !r.finished);
        if big {
            return match self.suspect {
                Some(previous) if now >= previous => {
                    self.suspect = None;
                    self.high_water = self.high_water.max(now);
                    Ok(self.high_water)
                }
                _ => {
                    self.suspect = Some(now);
                    Err(unavailable())
                }
            };
        }
        self.suspect = None;
        self.high_water = self.high_water.max(now);
        Ok(self.high_water)
    }

    /// Set the guard's time back to `now` after a wrong reading, and clamp what was dated
    /// under it.
    fn resync(&mut self, now: u64, policy: &SandboxPolicy) {
        self.high_water = now;
        self.suspect = None;
        let live_until = now.saturating_add(policy.bounds.max_ttl_ms);
        let traces_until = live_until.saturating_add(policy.result_retention_ms());
        for record in self.sandboxes.values_mut() {
            record.expires_at = record.expires_at.min(live_until);
            record.evict_at = record.evict_at.min(traces_until);
        }
        for usage in self.tenants.values_mut() {
            for at in usage.created_at.iter_mut() {
                *at = (*at).min(now);
            }
        }
    }

    /// Give the sandbox's slot back, once.
    fn release(&mut self, sandbox_id: &str) {
        if let Some(record) = self.sandboxes.get_mut(sandbox_id) {
            if record.slot_held {
                record.slot_held = false;
                if let Some(usage) = self.tenants.get_mut(record.ctx.tenant_id.as_str()) {
                    usage.active = usage.active.saturating_sub(1);
                }
            }
        }
    }

    fn live_records(&self, tenant: Option<&str>) -> usize {
        self.sandboxes
            .values()
            .filter(|r| !r.finished && tenant.is_none_or(|t| r.ctx.tenant_id.as_str() == t))
            .count()
    }

    /// Records plus the creates that will land one: the figure the registry bounds apply to.
    ///
    /// Known limit: a retry of a create whose first call is still at the backend holds a second
    /// reservation, so at a bound it is refused with `quota_exceeded` instead of replaying the
    /// handle. The tenant concurrency ceiling behaves the same way.
    fn reserved_records(&self, tenant: Option<&str>) -> usize {
        let pending = match tenant {
            Some(t) => self.tenants.get(t).map_or(0, |usage| usage.pending),
            None => self.pending_creates,
        };
        self.live_records(tenant).saturating_add(pending)
    }

    /// The retired domain (no live sandbox uses it) used least recently, of `tenant` or of
    /// anyone.
    fn retired_lru(&self, tenant: Option<&TenantId>) -> Option<String> {
        let live: std::collections::HashSet<&str> = self
            .sandboxes
            .values()
            .filter(|r| !r.finished)
            .map(|r| r.domain.as_str())
            .collect();
        self.domains
            .iter()
            .filter(|(domain, entry)| {
                !live.contains(domain.as_str()) && tenant.is_none_or(|t| entry.tenant == *t)
            })
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(domain, _)| domain.clone())
    }

    fn owned_domains(&self, tenant: &TenantId) -> usize {
        self.domains
            .values()
            .filter(|e| e.tenant == *tenant)
            .count()
    }

    /// Whether `tenant` could take one more domain, evicting a retired one if needed.
    fn domain_room(&self, tenant: &TenantId, policy: &SandboxPolicy) -> bool {
        if self.owned_domains(tenant) >= policy.max_domains_per_tenant
            && self.retired_lru(Some(tenant)).is_none()
        {
            return false;
        }
        self.domains.len() < policy.max_isolation_domains || self.retired_lru(None).is_some()
    }

    /// Make room for one new domain of `tenant`; `false` when no retired domain can leave.
    fn make_domain_room(&mut self, tenant: &TenantId, policy: &SandboxPolicy) -> bool {
        if self.owned_domains(tenant) >= policy.max_domains_per_tenant {
            match self.retired_lru(Some(tenant)) {
                Some(domain) => {
                    self.domains.remove(&domain);
                }
                None => return false,
            }
        }
        if self.domains.len() >= policy.max_isolation_domains {
            match self.retired_lru(None) {
                Some(domain) => {
                    self.domains.remove(&domain);
                }
                None => return false,
            }
        }
        true
    }

    /// Expire sandboxes past their `ttl`, compact finished records, evict the old ones and
    /// the oldest beyond a tenant's bound, forget idle tenants. Each call walks the whole
    /// registry (O(n) in its size, which the policy bounds); it runs once per `create`.
    fn sweep(&mut self, now: u64, policy: &SandboxPolicy) {
        let expired: Vec<String> = self
            .sandboxes
            .iter()
            .filter(|(_, r)| !r.finished && now >= r.expires_at)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.release(&id);
            if let Some(record) = self.sandboxes.get_mut(&id) {
                record.compact();
            }
        }
        let evicted: Vec<String> = self
            .sandboxes
            .iter()
            .filter(|(_, r)| r.finished && now >= r.evict_at)
            .map(|(id, _)| id.clone())
            .collect();
        for id in evicted {
            self.remove_record(&id);
        }
        // Finished traces are bounded per tenant: the oldest of an over-full tenant go.
        let mut per_tenant: HashMap<String, Vec<(u64, String)>> = HashMap::new();
        for (id, r) in self.sandboxes.iter().filter(|(_, r)| r.finished) {
            per_tenant
                .entry(r.ctx.tenant_id.as_str().to_owned())
                .or_default()
                .push((r.evict_at, id.clone()));
        }
        for mut traces in per_tenant.into_values() {
            if traces.len() > policy.max_records_per_tenant {
                traces.sort();
                let excess = traces.len() - policy.max_records_per_tenant;
                for (_, id) in traces.into_iter().take(excess) {
                    self.remove_record(&id);
                }
            }
        }
        let window_ms = policy.tenant_quota.window_ms;
        for usage in self.tenants.values_mut() {
            while usage
                .created_at
                .front()
                .is_some_and(|at| now.saturating_sub(*at) >= window_ms)
            {
                usage.created_at.pop_front();
            }
        }
        self.tenants
            .retain(|_, u| u.active > 0 || !u.created_at.is_empty());
    }

    fn remove_record(&mut self, id: &str) {
        if let Some(record) = self.sandboxes.remove(id) {
            self.creates.remove(&record.create_key);
        }
    }
}

/// A tenant slot reserved by `create`. Given back when dropped, unless it was kept: this
/// covers a future dropped mid-call, a panic and every error path.
struct SlotGuard<'a> {
    inner: &'a Mutex<Inner>,
    tenant: String,
    armed: bool,
}

impl SlotGuard<'_> {
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut inner) = self.inner.lock() {
                inner.pending_creates = inner.pending_creates.saturating_sub(1);
                if let Some(usage) = inner.tenants.get_mut(&self.tenant) {
                    usage.active = usage.active.saturating_sub(1);
                    usage.pending = usage.pending.saturating_sub(1);
                }
            }
        }
    }
}

/// Marks an `exec` as being at the backend; cleared when dropped.
struct FlightGuard<'a> {
    inner: &'a Mutex<Inner>,
    sandbox_id: String,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(record) = inner.sandboxes.get_mut(&self.sandbox_id) {
                record.in_flight = false;
            }
        }
    }
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

enum CreateStart<'a> {
    Replay(SandboxHandle),
    Run(SlotGuard<'a>),
}

enum Landed {
    Created(SandboxHandle),
    Replay(SandboxHandle),
    Violation {
        handle: SandboxHandle,
        signal: &'static str,
    },
    IdReused,
    /// No isolation domain can be added for the tenant.
    DomainsFull(SandboxHandle),
}

enum ExecStart<'a> {
    /// The call already has an answer: the recorded result, or the error that ends it.
    Replay(ExecResult),
    Run(FlightGuard<'a>),
}

enum CollectStart {
    Replay(Box<Collected>, Option<Box<ExecutionProof>>),
    Run {
        output_cap: u64,
        secret_grants: usize,
        /// Present when a signer is configured.
        parts: Option<Box<ProofParts>>,
    },
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

/// An error after which asking the backend again is pointless or unsafe. Only a wait, a
/// retriable unavailability and a cancellation honoured by the backend can be asked again.
fn is_replayable(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::Busy { .. }
            | SandboxError::Unavailable { retriable: true }
            | SandboxError::Cancelled
    )
}

/// Reduce a backend error to what the guard may relay: its code, and a fixed message. The
/// variants only the guard may raise become a generic backend error, so a backend cannot
/// forge a refusal, an approval request or a state error.
fn sanitize_backend_error(error: SandboxError) -> SandboxError {
    match error {
        SandboxError::Busy { retry_after_ms } => SandboxError::Busy { retry_after_ms },
        SandboxError::Unavailable { retriable } => SandboxError::Unavailable { retriable },
        SandboxError::Cancelled => SandboxError::Cancelled,
        SandboxError::LimitExceeded { kind, partial } => SandboxError::LimitExceeded {
            kind,
            partial: Box::new(sanitize_outcome(*partial)),
        },
        SandboxError::IsolationViolation { .. } => SandboxError::IsolationViolation {
            signal: SIGNAL_BACKEND.to_owned(),
        },
        SandboxError::InvalidPath { .. } => SandboxError::InvalidPath {
            reason: REJECTED_PATH_REASON.to_owned(),
        },
        SandboxError::Backend(_)
        | SandboxError::PolicyDenied(_)
        | SandboxError::ApprovalRequired { .. }
        | SandboxError::QuotaExceeded { .. }
        | SandboxError::IdempotencyConflict
        | SandboxError::NotFound
        | SandboxError::InvalidState { .. } => {
            SandboxError::Backend(GENERIC_BACKEND_MESSAGE.to_owned())
        }
    }
}

/// Reduce an outcome declared by a backend to bounded, plain values: the only free text in
/// it is a signal name, kept to a short word.
fn sanitize_outcome(mut outcome: ExecOutcome) -> ExecOutcome {
    if let DeclaredExit::Signal { name } = &mut outcome.declared_exit {
        *name = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(MAX_SIGNAL_CHARS)
            .collect();
    }
    outcome.truncation.truncate(MAX_TRUNCATION_MARKERS);
    outcome
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

/// The payload hash of the approval the verifier established, if any.
fn payload_hash_of(verification: &Verification) -> Option<String> {
    match verification {
        Some(Ok(grant)) => Some(grant.approval_payload_hash.clone()),
        _ => None,
    }
}

fn unavailable() -> SandboxError {
    SandboxError::Unavailable { retriable: true }
}

/// The registry lock was poisoned by a panic: nothing can be trusted, so fail closed.
fn poisoned() -> SandboxError {
    SandboxError::Unavailable { retriable: false }
}

/// What `create` asked for that the proof states, derived when a signer is configured.
fn proof_basis(req: &CreateRequest) -> ProofBasis {
    let mut inputs = Canon::new("sandbox.inputs.v1");
    inputs.num("count", req.inputs.len() as u64);
    for input in &req.inputs {
        inputs.json("input", input);
    }
    ProofBasis {
        profile_digest: sha256_hex(req.profile.as_bytes()),
        limits: req.limits.clone(),
        inputs_digest: inputs.finish(),
    }
}

/// Gather what the proof of `record`'s `collect` is made of. `None` when something is missing
/// (no definite outcome, no approval remembered, no basis): the guard then signs nothing.
fn proof_parts(record: &Record) -> Option<ProofParts> {
    let entry = record.exec.as_ref()?;
    let outcome = match entry.result.as_ref()? {
        Ok(outcome) => outcome.clone(),
        Err(SandboxError::LimitExceeded { partial, .. }) => (**partial).clone(),
        Err(_) => return None,
    };
    Some(ProofParts {
        sandbox_id: record.handle.sandbox_id().clone(),
        domain: record.domain.clone(),
        create_fingerprint: record.create_fingerprint.clone(),
        exec_fingerprint: entry.fingerprint.clone(),
        outcome,
        grant: entry.grant.clone()?,
        started_at: entry.started_at,
        finished_at: entry.finished_at?,
        basis: record.basis.clone()?,
        violation: record.violation,
    })
}

/// The proof of a `collect`. The digests of the streams and of the outputs are those the guard
/// computed over the bytes it holds; the exit and the duration are the backend's declaration;
/// the times are the guard's.
fn build_proof(
    ctx: &SandboxContext,
    key: &IdempotencyKey,
    parts: &ProofParts,
    collect_fingerprint: &str,
    collected: &Collected,
) -> ExecutionProof {
    let mut request = Canon::new("sandbox.proof.request.v1");
    request
        .str("create", &parts.create_fingerprint)
        .str("exec", &parts.exec_fingerprint)
        .str("collect", collect_fingerprint);
    ExecutionProof {
        kind: PROOF_KIND.to_owned(),
        run_id: ctx.run_id.as_str().to_owned(),
        node_id: ctx.node_id.as_str().to_owned(),
        tenant_id: ctx.tenant_id.as_str().to_owned(),
        sandbox_id: parts.sandbox_id.as_str().to_owned(),
        idempotency_key: key.as_str().to_owned(),
        isolation_domain: parts.domain.as_str().to_owned(),
        request_digest: request.finish(),
        profile_digest: parts.basis.profile_digest.clone(),
        limits_effective: parts.basis.limits.clone(),
        inputs_digest: parts.basis.inputs_digest.clone(),
        outputs_digest: collected.outputs_digest.clone(),
        stdout_digest: collected.stdout_sha256.clone(),
        stderr_digest: collected.stderr_sha256.clone(),
        truncated: parts.outcome.truncated || collected.truncated,
        declared_exit: parts.outcome.declared_exit.clone(),
        declared_duration_ms: parts.outcome.declared_duration_ms,
        started_at: parts.started_at,
        finished_at: parts.finished_at,
        violation: parts.violation,
        approval: ProofApproval {
            call_key: parts.grant.call_key.clone(),
            approved_by: parts.grant.approved_by.clone(),
        },
        approval_payload_hash: parts.grant.approval_payload_hash.clone(),
    }
}

/// The guard in front of one backend.
pub struct GovernedSandbox<B: SandboxBackend> {
    backend: B,
    policy: SandboxPolicy,
    clock: Arc<dyn SandboxClock>,
    journal: Arc<dyn SandboxJournal>,
    verifier: Arc<dyn GrantVerifier>,
    /// The signer of execution proofs. `None`: no proof is built and `collect` is as it was.
    signer: Option<Arc<dyn ProofSigner>>,
    inner: Mutex<Inner>,
    not_found: AtomicU64,
    /// A deliberate defect, to show that the conformance suite notices it (tests only).
    #[cfg(test)]
    fault: Option<ProofFault>,
}

/// Deliberate defects of the proof handling, for the tests of the conformance suite: each one is
/// a mistake the cases must catch.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProofFault {
    /// The result is recorded before it is signed, and a replay is served from the registry
    /// without a signature.
    SettleBeforeSign,
    /// A signer that fails is ignored: the result is returned with no proof.
    IgnoreSignerError,
    /// The record the signer returns is not compared with the proof that was submitted.
    SkipReturnedRecordCheck,
    /// The record the signer returns is checked for coherence (kind, algorithm, payload hash)
    /// but its content is not compared with the proof that was submitted.
    SkipContentCheck,
}

#[cfg(test)]
thread_local! {
    /// The defect the next guard built on this thread gets.
    pub(crate) static NEXT_FAULT: std::cell::Cell<Option<ProofFault>> =
        const { std::cell::Cell::new(None) };
}

impl<B: SandboxBackend> GovernedSandbox<B> {
    /// Build the guard. The policy is validated; one that would silently disable a ceiling is
    /// refused.
    pub fn new(
        backend: B,
        policy: SandboxPolicy,
        clock: Arc<dyn SandboxClock>,
        journal: Arc<dyn SandboxJournal>,
        verifier: Arc<dyn GrantVerifier>,
    ) -> Result<Self, PolicyConfigError> {
        policy.validate()?;
        policy.validate_for_backend(backend.max_retention())?;
        Ok(Self {
            backend,
            policy,
            clock,
            journal,
            verifier,
            signer: None,
            inner: Mutex::new(Inner::default()),
            not_found: AtomicU64::new(0),
            #[cfg(test)]
            fault: NEXT_FAULT.with(std::cell::Cell::get),
        })
    }

    /// Put a [`ProofSigner`] in front of `collect`: a capability the host opts into. Without
    /// one, the guard builds no proof, requires none and behaves exactly as before.
    ///
    /// With a signer, `collect` builds an [`ExecutionProof`] from what the guard recomputed
    /// over the bytes it received (never the digests the backend declared), has it signed,
    /// and only then records and returns the result. **No signed proof, no result:**
    ///
    /// - a signer that is unavailable gives `Unavailable { retriable: true }`;
    /// - a signer that refuses, or returns a record that does not say what was submitted, gives
    ///   `policy_denied.proof_refused`;
    /// - in both cases nothing is recorded or returned, and the sandbox stays collectable. The
    ///   program has already run and its effect stands: the backend keeps its result, and
    ///   asking again (the host's resume) collects it and asks for the signature again.
    ///
    /// A `collect` replayed after a success asks the signer again; a signer idempotent by key
    /// returns the same record. Use [`GovernedSandbox::collect_proved`] to receive the proof
    /// with the result; [`GovernedSandbox::collect`] signs too but returns the result only.
    ///
    /// The proof states the approval that let `exec` through, which the guard remembers from
    /// that call, since `collect` carries no claim.
    ///
    /// "Fail closed" covers a signer that does not answer or refuses. It does not cover a signer
    /// that signs badly: the guard holds no key and does not check signatures, only that the
    /// record is coherent and about the proof it submitted. A bad signature is found by the
    /// verifier ([`verify_proof_authentic`](crate::proof::verify_proof_authentic)) against its pinned keys.
    #[must_use]
    pub fn with_proof_signer(mut self, signer: Arc<dyn ProofSigner>) -> Self {
        self.signer = Some(signer);
        self
    }

    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// The window of an attempt for a run that starts at `run_started_at_ms`. The host calls
    /// this once, when the run starts, and keeps the result with the run: the retention in
    /// force at that instant is `min(guard retention, backend.max_retention())`.
    pub fn attempt_window(&self, run_started_at_ms: u64) -> AttemptWindow {
        AttemptWindow::new(
            run_started_at_ms,
            self.policy
                .effective_retention_ms(self.backend.max_retention()),
        )
    }

    /// The last instant an attempt of `window` is accepted, under the retention in force now.
    fn deadline_of(&self, window: &AttemptWindow) -> u64 {
        self.policy
            .attempt_deadline_ms(window, self.backend.max_retention())
    }

    /// The deadline handed to the backend: the start of the run plus the retention recorded at
    /// that instant, or the current one when none was recorded. Unlike [`Self::deadline_of`] it
    /// is not shortened by a retention that moved since: the backend keeps its state for the
    /// whole window the run was promised.
    fn backend_deadline_of(&self, window: &AttemptWindow) -> u64 {
        let retention = window.retention_at_start_ms.unwrap_or_else(|| {
            self.policy
                .effective_retention_ms(self.backend.max_retention())
        });
        window.run_started_at_ms.saturating_add(retention)
    }

    /// The age check, also made inside the registry step, on the instant the sweep uses, so
    /// that a replay cannot pass it after the state it would replay was swept.
    fn check_deadline(now: u64, deadline: u64) -> Result<(), Refusal> {
        if now > deadline {
            return Err(Refusal::bare(SandboxError::PolicyDenied(
                crate::error::PolicyReason::AttemptTooOld,
            )));
        }
        Ok(())
    }

    /// How many calls were answered `NotFound` (another tenant's handle, an unknown sandbox).
    /// Only a count is kept: no identifier, so guessing handles cannot fill any record.
    pub fn not_found_count(&self) -> u64 {
        self.not_found.load(Ordering::Relaxed)
    }

    fn inner(&self) -> Result<MutexGuard<'_, Inner>, SandboxError> {
        self.inner.lock().map_err(|_| poisoned())
    }

    // ---- approval ---------------------------------------------------------------------

    /// Describe the call a node is about to make, after validating both requests against the
    /// deployment policy. Nothing is created and the backend is not reached. The node files
    /// the result with the approval engine and suspends; once approved it passes an
    /// [`ApprovalClaim`] to [`GovernedSandbox::create`] and [`GovernedSandbox::exec`], and the
    /// guard has the injected [`GrantVerifier`] establish it.
    pub fn request_approval(
        &self,
        ctx: &SandboxContext,
        create: &CreateRequest,
        exec: &ExecRequest,
    ) -> Result<ApprovalRequest, SandboxError> {
        validate_context(ctx)?;
        self.policy.check_create(create)?;
        validate_exec(exec, &self.policy.bounds)?;
        // A call that cannot be shown whole is refused before anyone is asked to approve it.
        check_create_reviewable(create)?;
        check_exec_reviewable(exec)?;
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
            approval_payload_hash: None,
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

    /// Journal a refusal (when it is one the guard decided), count a `NotFound`, and hand the
    /// error back.
    async fn deny(
        &self,
        ctx: &SandboxContext,
        step: SandboxStep,
        refusal: Refusal,
    ) -> SandboxError {
        let Refusal { error, sandbox_id } = refusal;
        if matches!(error, SandboxError::NotFound) {
            self.not_found.fetch_add(1, Ordering::Relaxed);
        }
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
    /// `window` is a datum of the trusted caller (the instant the run started and the
    /// retention in force then, see [`GovernedSandbox::attempt_window`]; a bare `u64` start
    /// instant is accepted and records no retention); it is not covered by the call key, so
    /// the approval does not bind it.
    ///
    /// Validates the request against the policy and the age of the attempt
    /// (`window`), refuses a call that cannot be shown whole to an approver, has
    /// the verifier establish the approval for
    /// the exact call (`create` plus `planned`), admits the tenant, then asks the backend.
    /// The same context with the same request returns the handle already obtained; with
    /// another request it is an `IdempotencyConflict`.
    pub async fn create(
        &self,
        ctx: &SandboxContext,
        req: CreateRequest,
        planned: &ExecRequest,
        claim: Option<&ApprovalClaim>,
        window: impl Into<AttemptWindow>,
        cancel: &CancelToken,
    ) -> Result<SandboxHandle, SandboxError> {
        let window = window.into();
        let deadline = self.deadline_of(&window);
        let backend_deadline = self.backend_deadline_of(&window);
        let key = IdempotencyKey::new(ctx, SandboxStep::Create);
        let fingerprint = create_fingerprint(&req);
        // The age of the attempt is checked here, before the approval and the reservation, so
        // that a call the later `exec` would refuse as too old never gets that far.
        if let Err(error) = validate_context(ctx)
            .and_then(|()| self.policy.check_create(&req))
            .and_then(|()| validate_exec(planned, &self.policy.bounds))
            .and_then(|()| check_create_reviewable(&req))
            .and_then(|()| check_exec_reviewable(planned))
            .and_then(|()| {
                self.policy.check_attempt_age(
                    self.clock.now_ms(),
                    &window,
                    self.backend.max_retention(),
                )
            })
        {
            return Err(self
                .deny(ctx, SandboxStep::Create, Refusal::bare(error))
                .await);
        }
        let verification = verify_claim(self.verifier.as_ref(), ctx, claim).await;
        let payload_hash = payload_hash_of(&verification);
        // Time and cancellation are read before the registry lock is taken.
        let now = self.clock.now_ms();
        let cancelled = cancel.is_cancelled();
        let start = self.begin_create(
            ctx,
            &req,
            planned,
            &verification,
            (now, cancelled),
            deadline,
            &key,
            &fingerprint,
        );
        let slot = match start {
            Ok(CreateStart::Replay(handle)) => {
                self.emit_created(ctx, &key, &handle, &fingerprint).await?;
                return Ok(handle);
            }
            Ok(CreateStart::Run(slot)) => slot,
            Err(refusal) => return Err(self.deny(ctx, SandboxStep::Create, refusal).await),
        };

        let output_cap = req.limits.output_bytes;
        let secret_grants = req.env.len();
        let ttl_ms = req.ttl_ms;
        let subject_part = create_counts(&req);
        let basis = self.signer.as_ref().map(|_| proof_basis(&req));
        let raw = match self
            .backend
            .create(ctx, &key, req, backend_deadline, cancel)
            .await
        {
            Ok(raw) => raw,
            // `slot` is dropped here and gives the slot back.
            Err(error) => {
                // A violation the backend reports at creation is journaled as a denial.
                let refusal = Refusal::bare(sanitize_backend_error(error));
                return Err(self.deny(ctx, SandboxStep::Create, refusal).await);
            }
        };
        let landed = self.land_created(
            ctx,
            &key,
            &raw,
            &fingerprint,
            (subject_part, output_cap, secret_grants, ttl_ms, basis),
            payload_hash,
            slot,
        )?;
        match landed {
            Landed::Created(handle) | Landed::Replay(handle) => {
                self.emit_created(ctx, &key, &handle, &fingerprint).await?;
                Ok(handle)
            }
            Landed::Violation { handle, signal } => {
                // The caller cannot address this sandbox: tear it down here. Its id is new to
                // the guard, so this cannot touch another tenant's sandbox.
                self.destroy_poisoned(ctx, &handle).await;
                let error = SandboxError::IsolationViolation {
                    signal: signal.to_owned(),
                };
                let refusal = Refusal::new(error, Some(handle.sandbox_id().clone()));
                Err(self.deny(ctx, SandboxStep::Create, refusal).await)
            }
            Landed::DomainsFull(handle) => {
                // No record was made. The sandbox is new to the guard, so it can be
                // destroyed without touching anyone else's.
                self.destroy_poisoned(ctx, &handle).await;
                let error = SandboxError::QuotaExceeded {
                    kind: QuotaKind::IsolationDomainsFull,
                };
                Err(self
                    .deny(ctx, SandboxStep::Create, Refusal::bare(error))
                    .await)
            }
            Landed::IdReused => {
                // The id belongs to a sandbox the guard already knows. It cannot tell whose
                // resource this is, so it must not destroy it: the backend's `ttl` sweep
                // reclaims whatever this call left behind.
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
        verification: &Verification,
        (now, cancelled): (u64, bool),
        deadline: u64,
        key: &IdempotencyKey,
        fingerprint: &str,
    ) -> Result<CreateStart<'_>, Refusal> {
        let mut inner = self.inner().map_err(Refusal::bare)?;
        let now = inner.tick(now, &self.policy).map_err(Refusal::bare)?;
        Self::check_deadline(now, deadline)?;
        inner.sweep(now, &self.policy);
        if let Some(entry) = inner.creates.get(key.as_str()) {
            if entry.fingerprint != fingerprint {
                return Err(Refusal::new(
                    SandboxError::IdempotencyConflict,
                    Some(entry.sandbox_id.clone()),
                ));
            }
            return match inner.sandboxes.get(entry.sandbox_id.as_str()) {
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
        check_grant(verification.as_ref(), ctx, &expected, || {
            minimal_subject(ctx, &expected, &create_counts(req), &exec_counts(planned))
        })
        .map_err(Refusal::bare)?;
        if cancelled {
            return Err(Refusal::bare(SandboxError::Cancelled));
        }
        let quota_error = |kind| Refusal::bare(SandboxError::QuotaExceeded { kind });
        if inner.reserved_records(Some(ctx.tenant_id.as_str()))
            >= self.policy.max_records_per_tenant
        {
            return Err(quota_error(QuotaKind::TenantRegistryFull));
        }
        if !inner.domain_room(&ctx.tenant_id, &self.policy) {
            return Err(quota_error(QuotaKind::IsolationDomainsFull));
        }
        if inner.reserved_records(None) >= self.policy.max_registry_records {
            return Err(quota_error(QuotaKind::RegistryFull));
        }
        self.reserve_slot(&mut inner, ctx, now)
            .map_err(Refusal::bare)?;
        Ok(CreateStart::Run(SlotGuard {
            inner: &self.inner,
            tenant: ctx.tenant_id.as_str().to_owned(),
            armed: true,
        }))
    }

    /// Admission control, per tenant, answered at once. Counts the slot as taken.
    fn reserve_slot(
        &self,
        inner: &mut Inner,
        ctx: &SandboxContext,
        now: u64,
    ) -> Result<(), SandboxError> {
        let quota = &self.policy.tenant_quota;
        let usage = inner
            .tenants
            .entry(ctx.tenant_id.as_str().to_owned())
            .or_default();
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
        usage.pending += 1;
        usage.created_at.push_back(now);
        inner.pending_creates += 1;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn land_created(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        raw: &RawCreated,
        fingerprint: &str,
        (subject_part, output_cap, secret_grants, ttl_ms, basis): (
            Value,
            u64,
            usize,
            u64,
            Option<ProofBasis>,
        ),
        payload_hash: Option<String>,
        slot: SlotGuard<'_>,
    ) -> Result<Landed, SandboxError> {
        let now = self.clock.now_ms();
        let mut inner = self.inner()?;
        let now = inner.tick(now, &self.policy)?;
        // Another call with the same key landed while this one waited for the backend: the
        // slot this call reserved is dropped, the one of the first call stays.
        if let Some(entry) = inner.creates.get(key.as_str()) {
            if let Some(record) = inner.sandboxes.get(entry.sandbox_id.as_str()) {
                return Ok(Landed::Replay(record.handle.clone()));
            }
        }
        if inner.sandboxes.contains_key(raw.sandbox_id.as_str()) {
            return Ok(Landed::IdReused);
        }
        let handle = SandboxHandle::from_created(raw, &ctx.tenant_id);
        let violation = inner
            .domains
            .get(raw.isolation_domain.as_str())
            .is_some_and(|entry| entry.tenant != ctx.tenant_id);
        if !violation {
            let known = inner.domains.contains_key(raw.isolation_domain.as_str());
            if !known && !inner.make_domain_room(&ctx.tenant_id, &self.policy) {
                return Ok(Landed::DomainsFull(handle));
            }
            inner.domain_seq += 1;
            let last_used = inner.domain_seq;
            inner.domains.insert(
                raw.isolation_domain.as_str().to_owned(),
                DomainEntry {
                    tenant: ctx.tenant_id.clone(),
                    last_used,
                },
            );
        }
        let expires_at = now.saturating_add(ttl_ms);
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
                create_key: key.as_str().to_owned(),
                create_fingerprint: fingerprint.to_owned(),
                create_payload_hash: payload_hash,
                create_counts: subject_part,
                output_cap,
                secret_grants,
                expires_at,
                evict_at: expires_at.saturating_add(self.policy.result_retention_ms()),
                slot_held: true,
                closing: 0,
                finished: false,
                in_flight: false,
                exec: None,
                collect: None,
                basis,
                proof: None,
            },
        );
        // The record now owns the slot and replaces the reservation of registry capacity.
        inner.pending_creates = inner.pending_creates.saturating_sub(1);
        if let Some(usage) = inner.tenants.get_mut(ctx.tenant_id.as_str()) {
            usage.pending = usage.pending.saturating_sub(1);
        }
        slot.keep();
        Ok(if violation {
            Landed::Violation {
                handle,
                signal: SIGNAL_DOMAIN_SHARED,
            }
        } else {
            Landed::Created(handle)
        })
    }

    async fn emit_created(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        handle: &SandboxHandle,
        fingerprint: &str,
    ) -> Result<(), SandboxError> {
        // The approval linked to the event is the one that authorized the creation, kept in
        // the record, not the claim (if any) of a call that only replays it.
        let (domain, payload_hash) = {
            let inner = self.inner()?;
            let record = inner.sandboxes.get(handle.sandbox_id().as_str());
            (
                record.map(|record| record.domain.clone()),
                record.and_then(|record| record.create_payload_hash.clone()),
            )
        };
        let mut event = self.event(
            ctx,
            SandboxEventKind::SandboxCreated,
            key,
            Some(handle.sandbox_id()),
        );
        event.request_digest = Some(fingerprint.to_owned());
        event.isolation_domain = domain;
        event.approval_payload_hash = payload_hash;
        self.record(event).await
    }

    async fn destroy_poisoned(&self, ctx: &SandboxContext, handle: &SandboxHandle) {
        let key = IdempotencyKey::new(ctx, SandboxStep::Destroy);
        if self.backend.destroy(handle, &key).await.is_ok() {
            let _ = self.mark_destroyed(handle);
        }
    }

    /// The one transition to `Destroyed`: terminal, drops the collected bytes, and gives the
    /// slot back once (whatever else is still running against the sandbox).
    fn mark_destroyed(&self, handle: &SandboxHandle) -> Result<(), SandboxError> {
        let mut inner = self.inner()?;
        let id = handle.sandbox_id().as_str();
        if let Some(record) = inner.sandboxes.get_mut(id) {
            record.state = SandboxState::Destroyed;
            record.closing = record.closing.saturating_sub(1);
            record.compact();
        }
        inner.release(id);
        Ok(())
    }
}

// ---- exec ---------------------------------------------------------------------------

impl<B: SandboxBackend> GovernedSandbox<B> {
    /// Run the one program the sandbox may run.
    ///
    /// Before the backend: the context and the request bounds, the age of the attempt
    /// (`window` is the instant the run started and the retention in force then, a datum of the host), the handle
    /// (another tenant's handle is `NotFound`), the state, the approval for this exact call as
    /// established by the verifier, and cancellation.
    ///
    /// The same request again returns the recorded result without calling the backend; a
    /// different request on a sandbox that already has one is an `IdempotencyConflict`; one
    /// that is out of order is `InvalidState`. A second caller while the first is at the
    /// backend gets `Busy`.
    pub async fn exec(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: ExecRequest,
        claim: Option<&ApprovalClaim>,
        window: impl Into<AttemptWindow>,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        let window = window.into();
        let deadline = self.deadline_of(&window);
        let backend_deadline = self.backend_deadline_of(&window);
        let key = IdempotencyKey::new(ctx, SandboxStep::Exec);
        let fingerprint = exec_fingerprint(&req);
        let now = self.clock.now_ms();
        if let Err(error) = validate_context(ctx)
            .and_then(|()| validate_exec(&req, &self.policy.bounds))
            .and_then(|()| check_exec_reviewable(&req))
            .and_then(|()| {
                self.policy
                    .check_attempt_age(now, &window, self.backend.max_retention())
            })
        {
            return Err(self
                .deny(ctx, SandboxStep::Exec, Refusal::bare(error))
                .await);
        }
        let verification = verify_claim(self.verifier.as_ref(), ctx, claim).await;
        let payload_hash = payload_hash_of(&verification);
        // The verifier is asynchronous and may have taken long: the state of the sandbox and
        // the deadline are judged on the clock after it answered, not on the instant the call
        // started.
        let now = self.clock.now_ms();
        let cancelled = cancel.is_cancelled();
        let start = self.begin_exec(
            ctx,
            handle,
            &req,
            &fingerprint,
            &verification,
            (now, cancelled),
            deadline,
        );
        let _flight = match start {
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
            Ok(ExecStart::Run(flight)) => flight,
        };

        let mut started = self.event(
            ctx,
            SandboxEventKind::SandboxExecStarted,
            &key,
            Some(handle.sandbox_id()),
        );
        started.request_digest = Some(fingerprint.clone());
        started.approval_payload_hash = payload_hash;
        self.record(started).await?;
        // Appending to the journal may have suspended this call while the sandbox was
        // destroyed or expired: the program must not reach the backend first after that.
        if let Err(refusal) = self.still_open_for_exec(ctx, handle, deadline, cancel) {
            return Err(self.deny(ctx, SandboxStep::Exec, refusal).await);
        }

        let result = self
            .backend
            .exec(handle, &key, req, backend_deadline, cancel)
            .await
            .map(sanitize_outcome)
            .map_err(sanitize_backend_error);
        self.settle_exec(handle, &result)?;
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
        verification: &Verification,
        (now, cancelled): (u64, bool),
        deadline: u64,
    ) -> Result<ExecStart<'_>, Refusal> {
        let mut inner = self.inner().map_err(Refusal::bare)?;
        let now = inner.tick(now, &self.policy).map_err(Refusal::bare)?;
        Self::check_deadline(now, deadline)?;
        let record = Self::addressed(&mut inner, ctx, handle).map_err(Refusal::bare)?;
        let sandbox_id = record.handle.sandbox_id().clone();
        let refuse = |error| Refusal::new(error, Some(sandbox_id.clone()));
        let state = record.view(now);

        // The same key with another request is a conflict in every state.
        if record
            .exec
            .as_ref()
            .is_some_and(|entry| entry.fingerprint != fingerprint)
        {
            return Err(refuse(SandboxError::IdempotencyConflict));
        }
        match state {
            SandboxState::Created => {}
            SandboxState::Executing => {
                if let Some(terminal) = record.exec.as_ref().and_then(|e| e.terminal.clone()) {
                    return Ok(ExecStart::Replay(Err(terminal)));
                }
                if record.in_flight {
                    return Err(refuse(SandboxError::Busy {
                        retry_after_ms: Some(BUSY_RETRY_MS),
                    }));
                }
            }
            SandboxState::Executed | SandboxState::Collected => {
                return match record.exec.as_ref().and_then(|e| e.result.clone()) {
                    Some(result) => Ok(ExecStart::Replay(result)),
                    None => Err(refuse(SandboxError::InvalidState { from: state })),
                };
            }
            SandboxState::Poisoned | SandboxState::Destroyed => {
                return Err(refuse(SandboxError::InvalidState { from: state }));
            }
        }

        let expected = call_key(ctx, &record.create_fingerprint, fingerprint);
        check_grant(verification.as_ref(), ctx, &expected, || {
            minimal_subject(ctx, &expected, &record.create_counts, &exec_counts(req))
        })
        .map_err(refuse)?;
        if cancelled {
            return Err(refuse(SandboxError::Cancelled));
        }
        let trace = match verification {
            Some(Ok(grant)) => Some(GrantTrace {
                call_key: grant.call_key.clone(),
                approved_by: grant.approved_by.clone(),
                approval_payload_hash: grant.approval_payload_hash.clone(),
            }),
            _ => None,
        };
        record.state = SandboxState::Executing;
        record.in_flight = true;
        match record.exec.as_mut() {
            None => {
                record.exec = Some(ExecEntry {
                    fingerprint: fingerprint.to_owned(),
                    result: None,
                    result_digest: None,
                    terminal: None,
                    grant: trace,
                    started_at: now,
                    finished_at: None,
                });
            }
            // The call is made again after an error that left the outcome unknown: the
            // attempt that answers is the one that is dated.
            Some(entry) => {
                entry.grant = trace;
                entry.started_at = now;
                entry.finished_at = None;
            }
        }
        Ok(ExecStart::Run(FlightGuard {
            inner: &self.inner,
            sandbox_id: sandbox_id.as_str().to_owned(),
        }))
    }

    /// The sandbox is still there for an `exec` that is about to reach the backend: neither
    /// destroyed, being destroyed, expired nor poisoned, and the attempt is neither past its
    /// deadline nor cancelled. Read on the clock, under the lock.
    fn still_open_for_exec(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<(), Refusal> {
        // Time and cancellation are read before the registry lock is taken.
        let now = self.clock.now_ms();
        let cancelled = cancel.is_cancelled();
        let mut inner = self.inner().map_err(Refusal::bare)?;
        let now = inner.tick(now, &self.policy).map_err(Refusal::bare)?;
        // The journal may also have kept this call past the attempt deadline or a cancellation.
        Self::check_deadline(now, deadline)?;
        if cancelled {
            return Err(Refusal::bare(SandboxError::Cancelled));
        }
        let record = Self::addressed(&mut inner, ctx, handle).map_err(Refusal::bare)?;
        match record.view(now) {
            SandboxState::Executing => {
                // The program is about to be sent: this is the instant the proof states as
                // its start, on the guard's monotonic time and on every attempt, not the
                // instant the call began (the verifier and the journal may have taken time).
                if let Some(entry) = record.exec.as_mut() {
                    entry.started_at = now.max(entry.started_at);
                }
                Ok(())
            }
            state => Err(Refusal::new(
                SandboxError::InvalidState { from: state },
                Some(record.handle.sandbox_id().clone()),
            )),
        }
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

    /// Record what the backend answered. `Destroyed` is terminal: a result that comes back
    /// after a `destroy` is not recorded as a state change.
    fn settle_exec(&self, handle: &SandboxHandle, result: &ExecResult) -> Result<(), SandboxError> {
        // The clock is read before the registry lock is taken.
        let now = self.clock.now_ms();
        let mut inner = self.inner()?;
        // The proof dates the end of the run on the guard's own time, the one the sandbox
        // lifetimes are judged on, so that a clock that stepped back or jumped does not give
        // it a date the rest of the guard disagrees with. A reading the guard refuses to take
        // is not used: the end is then dated like the start.
        let ticked = inner.tick(now, &self.policy).ok();
        let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) else {
            return Ok(());
        };
        // `in_flight` is not cleared here: the call's `FlightGuard` owns it until the call
        // ends, so a second exec cannot start while this one still journals its end.
        // A record that is over (destroyed, or expired and compacted while the backend ran) keeps
        // no result: what the backend answers late is not recorded.
        if record.state == SandboxState::Destroyed || record.finished {
            return Ok(());
        }
        match result {
            Ok(_) | Err(SandboxError::LimitExceeded { .. }) => {
                record.state = SandboxState::Executed;
                if let Some(entry) = record.exec.as_mut() {
                    entry.result_digest = Some(outcome_digest(result));
                    entry.result = Some(result.clone());
                    entry.finished_at =
                        Some(ticked.unwrap_or(entry.started_at).max(entry.started_at));
                }
            }
            Err(SandboxError::IsolationViolation { .. }) => {
                record.state = SandboxState::Poisoned;
            }
            // The outcome is unknown, so the sandbox stays `Executing`: only the same call or
            // `destroy` can follow. An error that cannot be retried is remembered, and the
            // same call is answered with it instead of reaching the backend again.
            Err(error) => {
                if !is_replayable(error) {
                    if let Some(entry) = record.exec.as_mut() {
                        entry.terminal = Some(error.clone());
                    }
                }
            }
        }
        Ok(())
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
    /// redacted by the secret values the backend resolved (a backend that resolved fewer than
    /// were granted is refused), and the digests are recomputed by the guard over what it
    /// received.
    ///
    /// With a [`ProofSigner`] configured ([`GovernedSandbox::with_proof_signer`]), the result
    /// is signed before it is recorded or returned, and this method returns the result only;
    /// [`GovernedSandbox::collect_proved`] returns the proof with it. Without a signer nothing
    /// is signed and this is the behavior it always had.
    pub async fn collect(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: CollectRequest,
        window: impl Into<AttemptWindow>,
        cancel: &CancelToken,
    ) -> Result<Collected, SandboxError> {
        self.collect_inner(ctx, handle, req, window, cancel)
            .await
            .map(|(collected, _)| collected)
    }

    /// Like [`GovernedSandbox::collect`], and return the signed proof of the execution with the
    /// result. Needs a signer: a guard without one answers `Unavailable { retriable: false }`
    /// before doing anything, since it has no proof to give.
    ///
    /// The proof is signed before the result is recorded or returned, so a result never exists
    /// without it. If the signer is unavailable or refuses, the call fails (see
    /// [`GovernedSandbox::with_proof_signer`]); the program has already run, and the same call
    /// made again collects it and asks for the signature again. A replayed call returns the
    /// same record, for a signer that is idempotent by key.
    pub async fn collect_proved(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: CollectRequest,
        window: impl Into<AttemptWindow>,
        cancel: &CancelToken,
    ) -> Result<ProvedCollected, SandboxError> {
        if self.signer.is_none() {
            return Err(poisoned());
        }
        let (collected, proof) = self.collect_inner(ctx, handle, req, window, cancel).await?;
        // A result is never returned without its proof.
        let proof = proof.ok_or_else(unavailable)?;
        Ok(ProvedCollected { collected, proof })
    }

    async fn collect_inner(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: CollectRequest,
        window: impl Into<AttemptWindow>,
        cancel: &CancelToken,
    ) -> Result<(Collected, Option<SignedExecutionProof>), SandboxError> {
        let window = window.into();
        let deadline = self.deadline_of(&window);
        let backend_deadline = self.backend_deadline_of(&window);
        let key = IdempotencyKey::new(ctx, SandboxStep::Collect);
        let fingerprint = collect_fingerprint(&req);
        let now = self.clock.now_ms();
        if let Err(error) = validate_context(ctx)
            .and_then(|()| validate_collect(&req, &self.policy.bounds))
            .and_then(|()| {
                self.policy
                    .check_attempt_age(now, &window, self.backend.max_retention())
            })
        {
            return Err(self
                .deny(ctx, SandboxStep::Collect, Refusal::bare(error))
                .await);
        }
        let cancelled = cancel.is_cancelled();
        let start = self.begin_collect(ctx, handle, &fingerprint, (now, cancelled), deadline);
        let (output_cap, secret_grants, parts) = match start {
            Err(refusal) => return Err(self.deny(ctx, SandboxStep::Collect, refusal).await),
            Ok(CollectStart::Replay(collected, proof)) => {
                // A result recorded earlier is signed again before it is returned: the signer
                // answers with the record it already holds.
                let signed = match (&self.signer, proof) {
                    (None, _) => None,
                    #[cfg(test)]
                    (Some(_), _) if self.fault == Some(ProofFault::SettleBeforeSign) => None,
                    (Some(_), Some(proof)) => self.sign_proof(ctx, handle, &proof).await?,
                    (Some(_), None) => return Err(poisoned()),
                };
                self.emit_collected(ctx, &key, handle, &fingerprint, &collected)
                    .await?;
                return Ok((*collected, signed));
            }
            Ok(CollectStart::Run {
                output_cap,
                secret_grants,
                parts,
            }) => (output_cap, secret_grants, parts),
        };

        let raw = match self
            .backend
            .collect(handle, &key, req.clone(), backend_deadline, cancel)
            .await
        {
            Ok(raw) => raw,
            Err(error) => {
                let error = sanitize_backend_error(error);
                if matches!(error, SandboxError::IsolationViolation { .. }) {
                    self.poison(handle)?;
                    let refusal = Refusal::new(error, Some(handle.sandbox_id().clone()));
                    return Err(self.deny(ctx, SandboxStep::Collect, refusal).await);
                }
                return Err(error);
            }
        };
        let collected = derive(raw, output_cap, &req, secret_grants)?;
        let proof = parts
            .as_deref()
            .map(|parts| build_proof(ctx, &key, parts, &fingerprint, &collected));
        // Signed before it is recorded: a result with no signature never reaches the registry.
        #[cfg(test)]
        if self.fault == Some(ProofFault::SettleBeforeSign) {
            self.settle(handle, &fingerprint, &collected, proof.clone())?;
        }
        let signed = match &proof {
            Some(proof) => self.sign_proof(ctx, handle, proof).await?,
            None => None,
        };
        match proof {
            Some(proof) => self.settle(handle, &fingerprint, &collected, Some(proof))?,
            None => self.settle_collect(handle, &fingerprint, &collected)?,
        }
        self.emit_collected(ctx, &key, handle, &fingerprint, &collected)
            .await?;
        Ok((collected, signed))
    }

    /// Have the signer sign `proof`. Anything but a record that says what was submitted fails
    /// closed: an unavailable signer is `Unavailable { retriable: true }`, a refusal or a
    /// record that differs is `policy_denied.proof_refused`.
    async fn sign_proof(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        proof: &ExecutionProof,
    ) -> Result<Option<SignedExecutionProof>, SandboxError> {
        let Some(signer) = &self.signer else {
            return Ok(None);
        };
        let outcome = match signer.sign(proof).await {
            Ok(signed) if self.says_what_was_submitted(&signed, proof) => return Ok(Some(signed)),
            Ok(_) | Err(SignerError::Refused(_)) => {
                SandboxError::PolicyDenied(PolicyReason::ProofRefused)
            }
            Err(SignerError::Unavailable) => unavailable(),
        };
        #[cfg(test)]
        if self.fault == Some(ProofFault::IgnoreSignerError) {
            return Ok(None);
        }
        let refusal = Refusal::new(outcome, Some(handle.sandbox_id().clone()));
        Err(self.deny(ctx, SandboxStep::Collect, refusal).await)
    }

    /// Whether the record the signer returned is about the proof the guard submitted: the same
    /// content (the times of an earlier signing may differ), a sandbox proof, and a payload
    /// hash that is the hash of what it holds. The guard holds no key, so the signature is for
    /// the verifier.
    fn says_what_was_submitted(
        &self,
        signed: &SignedExecutionProof,
        proof: &ExecutionProof,
    ) -> bool {
        #[cfg(test)]
        if self.fault == Some(ProofFault::SkipReturnedRecordCheck) {
            return true;
        }
        #[cfg(test)]
        let same_content =
            self.fault == Some(ProofFault::SkipContentCheck) || signed.proof.same_content_as(proof);
        #[cfg(not(test))]
        let same_content = signed.proof.same_content_as(proof);
        signed.proof.kind == PROOF_KIND
            && signed.algorithm == PROOF_ALGORITHM
            && signed.domain == PROOF_DOMAIN
            && same_content
            && payload_hash(&signed.proof).as_deref() == Some(signed.payload_hash.as_str())
    }

    fn begin_collect(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        fingerprint: &str,
        (now, cancelled): (u64, bool),
        deadline: u64,
    ) -> Result<CollectStart, Refusal> {
        let mut inner = self.inner().map_err(Refusal::bare)?;
        let now = inner.tick(now, &self.policy).map_err(Refusal::bare)?;
        Self::check_deadline(now, deadline)?;
        let record = Self::addressed(&mut inner, ctx, handle).map_err(Refusal::bare)?;
        let refuse = |error| Refusal::new(error, Some(record.handle.sandbox_id().clone()));
        let state = record.view(now);
        if record
            .collect
            .as_ref()
            .is_some_and(|(recorded, _)| recorded != fingerprint)
        {
            return Err(refuse(SandboxError::IdempotencyConflict));
        }
        match state {
            SandboxState::Executed => {}
            SandboxState::Collected => {
                return match &record.collect {
                    Some((_, collected)) => Ok(CollectStart::Replay(
                        Box::new(collected.clone()),
                        record.proof.clone().map(Box::new),
                    )),
                    None => Err(refuse(SandboxError::InvalidState { from: state })),
                };
            }
            SandboxState::Created
            | SandboxState::Executing
            | SandboxState::Poisoned
            | SandboxState::Destroyed => {
                return Err(refuse(SandboxError::InvalidState { from: state }));
            }
        }
        if cancelled {
            return Err(refuse(SandboxError::Cancelled));
        }
        let parts = if self.signer.is_some() {
            // Without what the proof is made of, nothing can be signed: fail closed.
            Some(Box::new(
                proof_parts(record).ok_or_else(|| refuse(poisoned()))?,
            ))
        } else {
            None
        };
        Ok(CollectStart::Run {
            output_cap: record.output_cap,
            secret_grants: record.secret_grants,
            parts,
        })
    }

    /// `Destroyed` is terminal: a collection that returns after a `destroy` is dropped.
    fn settle_collect(
        &self,
        handle: &SandboxHandle,
        fingerprint: &str,
        collected: &Collected,
    ) -> Result<(), SandboxError> {
        self.settle(handle, fingerprint, collected, None)
    }

    /// Like `settle_collect`, remembering the proof that was signed for the result, so that a
    /// replayed `collect` can have it signed again.
    fn settle(
        &self,
        handle: &SandboxHandle,
        fingerprint: &str,
        collected: &Collected,
        proof: Option<ExecutionProof>,
    ) -> Result<(), SandboxError> {
        let mut inner = self.inner()?;
        if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            // A record that is over keeps no delivery: what the backend hands over late is not
            // stored in it.
            if record.state == SandboxState::Executed && !record.finished {
                record.state = SandboxState::Collected;
                record.collect = Some((fingerprint.to_owned(), collected.clone()));
                record.proof = proof;
            }
        }
        Ok(())
    }

    fn poison(&self, handle: &SandboxHandle) -> Result<(), SandboxError> {
        let mut inner = self.inner()?;
        if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
            if record.state != SandboxState::Destroyed {
                record.state = SandboxState::Poisoned;
            }
        }
        Ok(())
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
    /// `NotFound`. Once started, nothing else can start on the sandbox, and `Destroyed` is
    /// terminal.
    pub async fn destroy(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<(), SandboxError> {
        validate_context(ctx)?;
        if handle.tenant_id() != &ctx.tenant_id {
            self.not_found.fetch_add(1, Ordering::Relaxed);
            return Err(SandboxError::NotFound);
        }
        let key = IdempotencyKey::new(ctx, SandboxStep::Destroy);
        let already = {
            let mut inner = self.inner()?;
            match inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
                Some(record) if record.ctx != *ctx || record.handle != *handle => {
                    drop(inner);
                    self.not_found.fetch_add(1, Ordering::Relaxed);
                    return Err(SandboxError::NotFound);
                }
                Some(record) if record.state == SandboxState::Destroyed => true,
                Some(record) => {
                    record.closing += 1;
                    false
                }
                None => false,
            }
        };
        if !already {
            if let Err(error) = self.backend.destroy(handle, &key).await {
                if let Ok(mut inner) = self.inner() {
                    if let Some(record) = inner.sandboxes.get_mut(handle.sandbox_id().as_str()) {
                        record.closing = record.closing.saturating_sub(1);
                    }
                }
                // A violation the backend reports while destroying is journaled as a denial.
                let refusal = Refusal::new(
                    sanitize_backend_error(error),
                    Some(handle.sandbox_id().clone()),
                );
                return Err(self.deny(ctx, SandboxStep::Destroy, refusal).await);
            }
            self.mark_destroyed(handle)?;
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
    /// silent. If the returned future is dropped, or `f` panics, nothing can be awaited:
    /// the backend's `ttl` sweep removes the sandbox and the guard gives the tenant's slot
    /// back at the same instant.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_sandbox<'a, T, F>(
        &'a self,
        ctx: &'a SandboxContext,
        create: CreateRequest,
        planned: &ExecRequest,
        claim: Option<&ApprovalClaim>,
        window: impl Into<AttemptWindow>,
        cancel: &'a CancelToken,
        f: F,
    ) -> Result<T, SandboxError>
    where
        F: for<'s> FnOnce(&'s SandboxSession<'a, B>) -> SessionFuture<'s, T>,
    {
        let handle = self
            .create(ctx, create, planned, claim, window, cancel)
            .await?;
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
        claim: Option<&ApprovalClaim>,
        window: impl Into<AttemptWindow>,
    ) -> Result<ExecOutcome, SandboxError> {
        self.guard
            .exec(self.ctx, &self.handle, req, claim, window, self.cancel)
            .await
    }

    pub async fn collect(
        &self,
        req: CollectRequest,
        window: impl Into<AttemptWindow>,
    ) -> Result<Collected, SandboxError> {
        self.guard
            .collect(self.ctx, &self.handle, req, window, self.cancel)
            .await
    }

    /// Like [`SandboxSession::collect`], with the signed proof; see
    /// [`GovernedSandbox::collect_proved`].
    pub async fn collect_proved(
        &self,
        req: CollectRequest,
        window: impl Into<AttemptWindow>,
    ) -> Result<ProvedCollected, SandboxError> {
        self.guard
            .collect_proved(self.ctx, &self.handle, req, window, self.cancel)
            .await
    }
}

#[cfg(test)]
mod tests;
