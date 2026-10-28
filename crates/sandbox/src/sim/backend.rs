//! [`SimBackend`]: the in-memory reference backend.

use super::fault::{FaultNotApplicable, SimFault};
use super::fs::SimFs;
use super::net::SimNetwork;
use super::output::{SimOutput, SimStream};
use super::program::{SimInvocation, SimProgram, SimResult};
use super::secrets::SimSecrets;
use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::clock::SandboxClock;
use crate::collected::{RawCollected, RawFile, ResolvedSecrets};
use crate::digest::{sha256_hex, Canon};
use crate::error::{LimitKind, SandboxError, SandboxState};
use crate::ids::{
    IdempotencyKey, IsolationDomain, RawCreated, SandboxContext, SandboxHandle, SandboxId,
    SandboxStep, TenantId,
};
use crate::limits::Limits;
use crate::outcome::{DeclaredExit, ExecOutcome, Truncation};
use crate::path::LogicalPath;
use crate::policy::{DEFAULT_RESULT_RETENTION_MS, MAX_RESULT_RETENTION_MS};
use crate::request::{
    CollectRequest, CreateRequest, EnvGrant, ExecRequest, NetworkPolicy, SecretRef,
};
use crate::secret::{SecretError, SecretResolver};
use ailu_fs_backend::ArtifactRef;
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use zeroize::Zeroizing;

/// What a caller is told to wait before asking again while the same call is still running.
const BUSY_RETRY_MS: u64 = 10;
/// The virtual size of the output of the `HugeOutput` fault: a terabyte, never allocated.
const HUGE_OUTPUT_BYTES: u64 = 1 << 40;

type ExecResult = Result<ExecOutcome, SandboxError>;

/// Settings of a [`SimBackend`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimConfig {
    /// How long the durable store keeps `(key -> request digest, result)`. A backend that
    /// forgot a key sooner than the guard's result retention would run the program a second
    /// time, so a value below [`MAX_RESULT_RETENTION_MS`] (the highest retention a policy
    /// can ask for) is raised to it when the backend is built: the retention can be
    /// lengthened, never shortened below what any policy needs. Default: 24 hours, raised to
    /// that ceiling.
    pub retention_ms: u64,
    /// Most records one tenant may hold in the durable store at once. Beyond it `create`
    /// answers `Busy` until records expire.
    pub max_records_per_tenant: usize,
    /// The environment every program starts with, and nothing else: no variable of the host
    /// is inherited. Default: `LANG` and `PATH`.
    pub base_env: BTreeMap<String, String>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            retention_ms: DEFAULT_RESULT_RETENTION_MS,
            max_records_per_tenant: 4096,
            base_env: BTreeMap::from([
                ("LANG".to_owned(), "C".to_owned()),
                ("PATH".to_owned(), "/usr/bin".to_owned()),
            ]),
        }
    }
}

/// What a [`SimBackend::sweep`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Sandboxes past their `ttl`.
    pub sandboxes: usize,
    /// Durable records past the retention.
    pub records: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reserved,
    Executing,
    Executed,
    Poisoned,
}

impl Phase {
    fn state(self) -> SandboxState {
        match self {
            Phase::Reserved => SandboxState::Created,
            Phase::Executing => SandboxState::Executing,
            Phase::Executed => SandboxState::Executed,
            Phase::Poisoned => SandboxState::Poisoned,
        }
    }
}

struct Sandbox {
    tenant: TenantId,
    ctx: SandboxContext,
    limits: Limits,
    network: NetworkPolicy,
    grants: Vec<EnvGrant>,
    fs: SimFs,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: Vec<Truncation>,
    phase: Phase,
    expires_at: u64,
}

/// What the durable store keeps for a collection: serializable, bounded, and holding no
/// secret value.
struct StoredCollected {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    files: Vec<(LogicalPath, Vec<u8>)>,
    truncated: bool,
    ctx: SandboxContext,
    grants: Vec<SecretRef>,
}

enum Stored {
    /// The execution marker: the call began, no result was kept.
    Marker,
    Created(RawCreated),
    Exec(ExecResult),
    Collected(Box<StoredCollected>),
}

/// One record of the durable store of a tenant.
struct Record {
    fingerprint: String,
    sandbox_id: String,
    expires_at: u64,
    /// An execution is under way right now (an execution resource exists).
    running: bool,
    stored: Stored,
}

#[derive(Default)]
struct State {
    programs: HashMap<String, SimProgram>,
    scripts: HashMap<SandboxStep, VecDeque<SimFault>>,
    artifacts: HashMap<(TenantId, ArtifactRef), Vec<u8>>,
    sandboxes: HashMap<String, Sandbox>,
    /// The durable store, isolated per tenant: a tenant's key is looked up in its own table
    /// only.
    records: HashMap<TenantId, HashMap<String, Record>>,
    /// Who owns each isolation domain this backend handed out.
    domains: HashMap<String, TenantId>,
    next_sandbox: u64,
    next_domain: u64,
}

impl State {
    fn record(&self, tenant: &TenantId, key: &str, now: u64) -> Option<&Record> {
        self.records
            .get(tenant)?
            .get(key)
            .filter(|record| now < record.expires_at)
    }

    fn sandbox(&self, handle: &SandboxHandle, now: u64) -> Result<&Sandbox, SandboxError> {
        self.sandboxes
            .get(handle.sandbox_id().as_str())
            .filter(|sb| sb.tenant == *handle.tenant_id() && now < sb.expires_at)
            .ok_or(SandboxError::NotFound)
    }

    fn sandbox_mut(
        &mut self,
        handle: &SandboxHandle,
        now: u64,
    ) -> Result<&mut Sandbox, SandboxError> {
        self.sandboxes
            .get_mut(handle.sandbox_id().as_str())
            .filter(|sb| sb.tenant == *handle.tenant_id() && now < sb.expires_at)
            .ok_or(SandboxError::NotFound)
    }

    fn pop_fault(&mut self, step: SandboxStep) -> Option<SimFault> {
        self.scripts.get_mut(&step).and_then(VecDeque::pop_front)
    }

    fn live_records(&self, tenant: &TenantId, now: u64) -> usize {
        self.records
            .get(tenant)
            .map_or(0, |r| r.values().filter(|r| now < r.expires_at).count())
    }

    fn put(&mut self, tenant: &TenantId, key: &str, record: Record) {
        self.records
            .entry(tenant.clone())
            .or_default()
            .insert(key.to_owned(), record);
    }
}

struct Shared {
    clock: Arc<dyn SandboxClock>,
    secrets: Arc<dyn SecretResolver>,
    network: Arc<SimNetwork>,
    config: SimConfig,
    state: Mutex<State>,
    program_calls: AtomicU64,
}

/// The reference backend. It runs no program: a registered Rust function answers for the
/// command, over a virtual filesystem, a virtual clock and a table of simulated hosts.
///
/// What it implements is the contract of [`SandboxBackend`], including every obligation that
/// trait lists:
///
/// - **Deduplication on the key, durable per tenant.** Each tenant has its own table of
///   `key -> (request digest, result)`, kept for [`SimConfig::retention_ms`]. The same key
///   with the same request answers what was stored without calling the program again; with
///   another request it is `IdempotencyConflict`. A replay works after `destroy`, and from a
///   guard that was restarted, as long as the retention lasts.
/// - **An execution marker, written before the program runs.** A marker without a result and
///   without an execution under way answers `Unavailable { retriable: false }` forever; the
///   program is never run a second time. A call that finds the execution still under way
///   gets `Busy`. (The simulator does not make the second caller wait for the result; it
///   answers `Busy` and lets it ask again.)
/// - **One isolation domain per sandbox**, never shared between tenants and never reused,
///   unless the `SharedDomain` fault asks for the opposite.
/// - **`NotFound`** for a handle that is another tenant's, expired or destroyed.
/// - **One `exec` per sandbox**, bounded streams, cancellation consulted before any effect.
/// - **A sweep on `ttl`**, driven by the injected clock: [`SimBackend::sweep`] removes what
///   is past its `ttl` or its retention, and [`SimBackend::residue`] counts what it would
///   remove. Past its `ttl` a sandbox already answers `NotFound`.
///
/// It is a cheap handle: clones share one state, so a test keeps a clone to register
/// programs, inject faults and read counters while the guard owns the other.
///
/// `Debug` shows counts only.
#[derive(Clone)]
pub struct SimBackend {
    shared: Arc<Shared>,
}

impl SimBackend {
    /// A backend with default settings, an empty secret store and an empty network.
    pub fn new(clock: Arc<dyn SandboxClock>) -> Self {
        Self::with_parts(
            clock,
            Arc::new(SimSecrets::new()),
            Arc::new(SimNetwork::new()),
            SimConfig::default(),
        )
    }

    /// A backend over the given secret store, network and settings. Keep a handle on the
    /// store and the network to fill them.
    pub fn with_parts(
        clock: Arc<dyn SandboxClock>,
        secrets: Arc<dyn SecretResolver>,
        network: Arc<SimNetwork>,
        mut config: SimConfig,
    ) -> Self {
        config.retention_ms = config.retention_ms.max(MAX_RESULT_RETENTION_MS);
        Self {
            shared: Arc::new(Shared {
                clock,
                secrets,
                network,
                config,
                state: Mutex::new(State::default()),
                program_calls: AtomicU64::new(0),
            }),
        }
    }

    /// A poisoned lock means a panic happened while the state was being changed: nothing can
    /// be trusted, so the backend fails closed.
    fn lock(&self) -> Result<MutexGuard<'_, State>, SandboxError> {
        self.shared
            .state
            .lock()
            .map_err(|_| SandboxError::Unavailable { retriable: false })
    }

    /// Configuration calls: a test helper must not fail because an earlier test panicked.
    fn lock_config(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a program under the name callers put in `argv[0]`, replacing one of the same
    /// name.
    pub fn register<F>(&self, name: &str, program: F)
    where
        F: Fn(&SimInvocation) -> SimResult + Send + Sync + 'static,
    {
        self.lock_config()
            .programs
            .insert(name.to_owned(), Arc::new(program));
    }

    /// Queue a fault for the next call of `step`. Faults are used in order, one per call
    /// that gets past the replay of a stored answer.
    pub fn inject(&self, step: SandboxStep, fault: SimFault) -> Result<(), FaultNotApplicable> {
        if !fault.applies_to(step) {
            return Err(FaultNotApplicable);
        }
        self.lock_config()
            .scripts
            .entry(step)
            .or_default()
            .push_back(fault);
        Ok(())
    }

    /// Forget every queued fault.
    pub fn clear_faults(&self) {
        self.lock_config().scripts.clear();
    }

    /// Make an artifact available to `tenant` as an input. Another tenant asking for the same
    /// reference gets `NotFound`.
    pub fn provide_artifact(
        &self,
        tenant: &TenantId,
        reference: ArtifactRef,
        bytes: impl Into<Vec<u8>>,
    ) {
        self.lock_config()
            .artifacts
            .insert((tenant.clone(), reference), bytes.into());
    }

    /// The simulated network, for its counters.
    pub fn network(&self) -> &SimNetwork {
        &self.shared.network
    }

    /// How long the durable store keeps a record (after the lower bound was applied).
    pub fn retention_ms(&self) -> u64 {
        self.shared.config.retention_ms
    }

    /// The environment every program starts with.
    pub fn base_env(&self) -> &BTreeMap<String, String> {
        &self.shared.config.base_env
    }

    /// How many times a program function was called, all programs together.
    pub fn program_calls(&self) -> u64 {
        self.shared.program_calls.load(Ordering::SeqCst)
    }

    /// How many sandboxes exist right now, expired ones included until they are swept.
    pub fn live_sandboxes(&self) -> usize {
        self.lock_config().sandboxes.len()
    }

    /// How many records the durable store holds for `tenant`, expired ones included until
    /// they are swept.
    pub fn stored_records(&self, tenant: &TenantId) -> usize {
        self.lock_config()
            .records
            .get(tenant)
            .map_or(0, HashMap::len)
    }

    /// Who owns an isolation domain this backend handed out.
    pub fn domain_owner(&self, domain: &str) -> Option<TenantId> {
        self.lock_config().domains.get(domain).cloned()
    }

    /// What a sweep would remove right now: sandboxes past their `ttl` and records past the
    /// retention. A conformance check fails if this is not zero after a sweep.
    pub fn residue(&self) -> usize {
        let now = self.shared.clock.now_ms();
        let st = self.lock_config();
        let sandboxes = st
            .sandboxes
            .values()
            .filter(|s| now >= s.expires_at)
            .count();
        let records = st
            .records
            .values()
            .flat_map(HashMap::values)
            .filter(|r| now >= r.expires_at)
            .count();
        sandboxes + records
    }

    /// Remove what is past its `ttl` (sandboxes) or its retention (durable records), as the
    /// clock reads now. Nothing else is touched.
    pub fn sweep(&self) -> SweepReport {
        let now = self.shared.clock.now_ms();
        let mut st = self.lock_config();
        let before = st.sandboxes.len();
        st.sandboxes.retain(|_, sb| now < sb.expires_at);
        let mut records = 0;
        for table in st.records.values_mut() {
            let before = table.len();
            table.retain(|_, r| now < r.expires_at || r.running);
            records += before - table.len();
        }
        st.records.retain(|_, table| !table.is_empty());
        SweepReport {
            sandboxes: before - st.sandboxes.len(),
            records,
        }
    }
}

impl std::fmt::Debug for SimBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.lock_config();
        f.debug_struct("SimBackend")
            .field("programs", &st.programs.len())
            .field("sandboxes", &st.sandboxes.len())
            .field("tenants", &st.records.len())
            .field("program_calls", &self.program_calls())
            .finish()
    }
}

// ---- fingerprints ---------------------------------------------------------------------

fn fingerprint<T: serde::Serialize>(domain: &str, request: &T) -> String {
    let mut canon = Canon::new(domain);
    canon.json("request", request);
    canon.finish()
}

fn secret_error(error: SecretError) -> SandboxError {
    match error {
        SecretError::NotFound => SandboxError::NotFound,
        SecretError::Unavailable => SandboxError::Unavailable { retriable: true },
    }
}

fn cancelled(cancel: &CancelToken) -> Result<(), SandboxError> {
    if cancel.is_cancelled() {
        Err(SandboxError::Cancelled)
    } else {
        Ok(())
    }
}

/// A key and a handle must belong to the same tenant, and the key must be the one of the
/// operation being asked, or the call is `NotFound`.
fn check_key(
    handle: &SandboxHandle,
    key: &IdempotencyKey,
    step: SandboxStep,
) -> Result<(), SandboxError> {
    if handle.tenant_id() == key.tenant_id() && key.step() == step {
        Ok(())
    } else {
        Err(SandboxError::NotFound)
    }
}

fn is_unknown_outcome(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::Busy { .. } | SandboxError::Unavailable { .. }
    )
}

// ---- create ---------------------------------------------------------------------------

impl SimBackend {
    fn do_create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
    ) -> Result<RawCreated, SandboxError> {
        if key.tenant_id() != &ctx.tenant_id || key.step() != SandboxStep::Create {
            return Err(SandboxError::NotFound);
        }
        let now = self.shared.clock.now_ms();
        let fp = fingerprint("sim.create.v1", &req);
        let tenant = &ctx.tenant_id;
        let mut st = self.lock()?;
        if let Some(record) = st.record(tenant, key.as_str(), now) {
            if record.fingerprint != fp {
                return Err(SandboxError::IdempotencyConflict);
            }
            return match &record.stored {
                Stored::Created(raw) => Ok(raw.clone()),
                _ => Err(SandboxError::Unavailable { retriable: false }),
            };
        }
        let mut share_domain = false;
        match st.pop_fault(SandboxStep::Create) {
            Some(SimFault::Error(error)) => return Err(error),
            Some(SimFault::SharedDomain) => share_domain = true,
            _ => {}
        }
        if st.live_records(tenant, now) >= self.shared.config.max_records_per_tenant {
            return Err(SandboxError::Busy {
                retry_after_ms: None,
            });
        }

        // Materialize the inputs: an artifact the tenant does not hold is `NotFound`.
        let fs = SimFs::new(req.limits.ephemeral_disk_bytes);
        for input in &req.inputs {
            let bytes = st
                .artifacts
                .get(&(tenant.clone(), input.artifact.clone()))
                .ok_or(SandboxError::NotFound)?;
            fs.write(input.dest.as_str(), bytes.clone())
                .map_err(|_| SandboxError::Backend("inputs do not fit the disk".to_owned()))?;
        }

        let sandbox_id = format!("sim-sb-{}", st.next_sandbox);
        st.next_sandbox += 1;
        let foreign = st
            .domains
            .iter()
            .filter(|(_, owner)| *owner != tenant)
            .map(|(domain, _)| domain.clone())
            .min();
        let domain = match (share_domain, foreign) {
            (true, Some(domain)) => domain,
            (true, None) => {
                // Nobody else holds a domain yet: hand out one that the next tenant to ask
                // for the same fault will find taken.
                let domain = "sim-domain-shared".to_owned();
                st.domains.insert(domain.clone(), tenant.clone());
                domain
            }
            (false, _) => {
                let domain = format!("sim-domain-{}", st.next_domain);
                st.next_domain += 1;
                st.domains.insert(domain.clone(), tenant.clone());
                domain
            }
        };
        let raw = RawCreated {
            sandbox_id: SandboxId::new(sandbox_id.clone()),
            isolation_domain: IsolationDomain::new(domain),
        };
        st.sandboxes.insert(
            sandbox_id.clone(),
            Sandbox {
                tenant: tenant.clone(),
                ctx: ctx.clone(),
                limits: req.limits.clone(),
                network: req.network.clone(),
                grants: req.env.clone(),
                fs,
                stdout: Vec::new(),
                stderr: Vec::new(),
                truncated: Vec::new(),
                phase: Phase::Reserved,
                expires_at: now.saturating_add(req.ttl_ms),
            },
        );
        st.put(
            tenant,
            key.as_str(),
            Record {
                fingerprint: fp,
                sandbox_id,
                expires_at: now.saturating_add(self.shared.config.retention_ms),
                running: false,
                stored: Stored::Created(raw.clone()),
            },
        );
        Ok(raw)
    }
}

// ---- exec -----------------------------------------------------------------------------

enum Gate {
    /// The call has an answer already: replay it, do not run.
    Replay(ExecResult),
    /// The call is new and may start.
    Proceed {
        ctx: SandboxContext,
        grants: Vec<EnvGrant>,
        program: SimProgram,
    },
}

struct RunPlan {
    program: SimProgram,
    invocation: SimInvocation,
    fault: Option<SimFault>,
    limits: Limits,
    request: ExecRequest,
}

enum Begun {
    Replay(ExecResult),
    Run(Box<RunPlan>),
}

/// What became of a run.
enum Verdict {
    Done(ExecOutcome),
    Killed {
        kind: LimitKind,
        partial: ExecOutcome,
    },
    /// A final error: it is kept, and replayed.
    Definite(SandboxError),
    /// The outcome is unknown: the marker stays, no result is kept.
    Lost(SandboxError),
}

struct Judged {
    verdict: Verdict,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncation: Vec<Truncation>,
}

fn echo_error(request: &ExecRequest) -> SandboxError {
    let stdin = String::from_utf8_lossy(request.stdin.as_deref().unwrap_or_default());
    SandboxError::Backend(format!("cannot run {:?} with input {stdin}", request.argv))
}

/// Decide how a run ended: apply the fault, if any, then the limits, then the output bounds.
fn judge(
    ran: std::thread::Result<SimResult>,
    script: Option<SimFault>,
    limits: &Limits,
    fs_used: u64,
    fs_exhausted: bool,
    request: &ExecRequest,
) -> Judged {
    let (mut output, raised) = match ran {
        Ok(Ok(output)) => (output, None),
        Ok(Err(fault)) => (SimOutput::success(), Some(fault)),
        Err(_) => {
            return Judged {
                verdict: Verdict::Lost(SandboxError::Backend(
                    "simulated program panicked".to_owned(),
                )),
                stdout: Vec::new(),
                stderr: Vec::new(),
                truncation: Vec::new(),
            }
        }
    };
    let fault = raised.or(script);
    if matches!(fault, Some(SimFault::HugeOutput)) {
        output.stdout = SimStream::Repeated {
            byte: b'x',
            len: HUGE_OUTPUT_BYTES,
        };
    }
    let cap = limits.output_bytes;
    let (stdout, cut_out) = output.stdout.take(cap);
    let (stderr, cut_err) = output.stderr.take(cap);
    let mut truncation = Vec::new();
    if cut_out {
        truncation.push(Truncation::Stdout);
    }
    if cut_err {
        truncation.push(Truncation::Stderr);
    }
    let outcome = |exit: DeclaredExit, duration: u64| ExecOutcome {
        declared_exit: exit,
        truncated: !truncation.is_empty(),
        truncation: truncation.clone(),
        declared_duration_ms: duration,
    };
    let killed = |kind: LimitKind| {
        let duration = match kind {
            LimitKind::WallTime => limits.wall_time_ms,
            _ => output.usage.duration_ms.min(limits.wall_time_ms),
        };
        Verdict::Killed {
            kind,
            partial: outcome(DeclaredExit::Killed { cause: kind }, duration),
        }
    };
    let verdict = match fault {
        Some(SimFault::Limit(kind)) => killed(kind),
        Some(SimFault::CrashAfterMarker) => {
            Verdict::Lost(SandboxError::Unavailable { retriable: true })
        }
        Some(SimFault::EchoRequest) => Verdict::Definite(echo_error(request)),
        Some(SimFault::Error(error)) if is_unknown_outcome(&error) => Verdict::Lost(error),
        Some(SimFault::Error(error)) => Verdict::Definite(error),
        Some(SimFault::SharedDomain | SimFault::ForgedOutput | SimFault::FewerSecrets) => {
            Verdict::Definite(SandboxError::Backend(
                "fault does not apply to exec".to_owned(),
            ))
        }
        Some(SimFault::HugeOutput) | None => {
            let usage = &output.usage;
            if usage.duration_ms > limits.wall_time_ms {
                killed(LimitKind::WallTime)
            } else if usage.memory_bytes > limits.memory_bytes {
                killed(LimitKind::Memory)
            } else if usage.processes > limits.max_processes {
                killed(LimitKind::Processes)
            } else if fs_exhausted
                || usage.disk_bytes.saturating_add(fs_used) > limits.ephemeral_disk_bytes
            {
                killed(LimitKind::Disk)
            } else {
                Verdict::Done(outcome(output.exit.clone(), usage.duration_ms))
            }
        }
    };
    Judged {
        verdict,
        stdout,
        stderr,
        truncation,
    }
}

impl SimBackend {
    /// Read-only checks shared by the two phases of `exec`.
    fn exec_gate(
        st: &State,
        handle: &SandboxHandle,
        key: &IdempotencyKey,
        fp: &str,
        now: u64,
        request: &ExecRequest,
    ) -> Result<Gate, SandboxError> {
        check_key(handle, key, SandboxStep::Exec)?;
        if let Some(record) = st.record(handle.tenant_id(), key.as_str(), now) {
            if record.sandbox_id != handle.sandbox_id().as_str() {
                return Err(SandboxError::NotFound);
            }
            if record.fingerprint != fp {
                return Err(SandboxError::IdempotencyConflict);
            }
            return match &record.stored {
                Stored::Exec(result) => Ok(Gate::Replay(result.clone())),
                _ if record.running => Err(SandboxError::Busy {
                    retry_after_ms: Some(BUSY_RETRY_MS),
                }),
                // The marker is there, the result is not and nothing is running: the program
                // may or may not have run. It is never run again.
                _ => Err(SandboxError::Unavailable { retriable: false }),
            };
        }
        let sandbox = st.sandbox(handle, now)?;
        if sandbox.phase != Phase::Reserved {
            return Err(SandboxError::InvalidState {
                from: sandbox.phase.state(),
            });
        }
        let program = request
            .argv
            .first()
            .and_then(|name| st.programs.get(name))
            .cloned()
            .ok_or_else(|| SandboxError::Backend("unknown program".to_owned()))?;
        Ok(Gate::Proceed {
            ctx: sandbox.ctx.clone(),
            grants: sandbox.grants.clone(),
            program,
        })
    }

    /// Second phase: with the secrets in hand, check again (another call may have started in
    /// the meantime), apply a scripted refusal, write the marker, and take the filesystem.
    fn exec_begin(
        &self,
        handle: &SandboxHandle,
        key: &IdempotencyKey,
        fp: &str,
        request: &ExecRequest,
        secrets: Vec<(String, Zeroizing<String>)>,
    ) -> Result<Begun, SandboxError> {
        let now = self.shared.clock.now_ms();
        let mut st = self.lock()?;
        let program = match Self::exec_gate(&st, handle, key, fp, now, request)? {
            Gate::Replay(result) => return Ok(Begun::Replay(result)),
            Gate::Proceed { program, .. } => program,
        };
        let mut fault = None;
        if let Some(scripted) = st.pop_fault(SandboxStep::Exec) {
            match scripted {
                SimFault::Error(error) => {
                    if matches!(error, SandboxError::IsolationViolation { .. }) {
                        st.sandbox_mut(handle, now)?.phase = Phase::Poisoned;
                    }
                    return Err(error);
                }
                other => fault = Some(other),
            }
        }

        // The marker goes in before anything runs.
        st.put(
            handle.tenant_id(),
            key.as_str(),
            Record {
                fingerprint: fp.to_owned(),
                sandbox_id: handle.sandbox_id().as_str().to_owned(),
                expires_at: now.saturating_add(self.shared.config.retention_ms),
                running: true,
                stored: Stored::Marker,
            },
        );
        let sandbox = st.sandbox_mut(handle, now)?;
        sandbox.phase = Phase::Executing;
        let disk = sandbox.limits.ephemeral_disk_bytes;
        let fs = std::mem::replace(&mut sandbox.fs, SimFs::new(disk));
        let mut env: BTreeMap<String, Zeroizing<String>> = self
            .shared
            .config
            .base_env
            .iter()
            .map(|(name, value)| (name.clone(), Zeroizing::new(value.clone())))
            .collect();
        env.extend(secrets);
        let invocation = SimInvocation::new(
            request.argv.clone(),
            request.cwd.as_ref().map(|p| p.as_str().to_owned()),
            request.stdin.clone().unwrap_or_default(),
            env,
            sandbox.limits.clone(),
            fs,
            sandbox.network.clone(),
            Arc::clone(&self.shared.network),
            now,
        );
        Ok(Begun::Run(Box::new(RunPlan {
            program,
            invocation,
            fault,
            limits: sandbox.limits.clone(),
            request: request.clone(),
        })))
    }

    /// Run the program. Synchronous, outside the lock, and the only place a program function
    /// is called.
    fn run(&self, plan: RunPlan) -> (Judged, SimFs) {
        self.shared.program_calls.fetch_add(1, Ordering::SeqCst);
        let RunPlan {
            program,
            invocation,
            fault,
            limits,
            request,
        } = plan;
        let ran = catch_unwind(AssertUnwindSafe(|| program(&invocation)));
        let fs = invocation.into_fs();
        let judged = judge(ran, fault, &limits, fs.used(), fs.exhausted(), &request);
        (judged, fs)
    }

    /// Keep the result durably and update the sandbox, if it still exists.
    fn exec_finish(
        &self,
        handle: &SandboxHandle,
        key: &IdempotencyKey,
        judged: Judged,
        fs: SimFs,
    ) -> ExecResult {
        let mut st = self.lock()?;
        let Judged {
            verdict,
            stdout,
            stderr,
            truncation,
        } = judged;
        let result: ExecResult = match &verdict {
            Verdict::Done(outcome) => Ok(outcome.clone()),
            Verdict::Killed { kind, partial } => Err(SandboxError::LimitExceeded {
                kind: *kind,
                partial: Box::new(partial.clone()),
            }),
            Verdict::Definite(error) | Verdict::Lost(error) => Err(error.clone()),
        };
        if let Some(record) = st
            .records
            .get_mut(handle.tenant_id())
            .and_then(|table| table.get_mut(key.as_str()))
        {
            record.running = false;
            if !matches!(verdict, Verdict::Lost(_)) {
                record.stored = Stored::Exec(result.clone());
            }
        }
        if let Some(sandbox) = st
            .sandboxes
            .get_mut(handle.sandbox_id().as_str())
            .filter(|sb| sb.tenant == *handle.tenant_id())
        {
            sandbox.fs = fs;
            sandbox.stdout = stdout;
            sandbox.stderr = stderr;
            sandbox.truncated = truncation;
            sandbox.phase = match &verdict {
                Verdict::Done(_) | Verdict::Killed { .. } => Phase::Executed,
                Verdict::Definite(SandboxError::IsolationViolation { .. }) => Phase::Poisoned,
                _ => Phase::Executing,
            };
        }
        result
    }

    async fn do_exec(
        &self,
        handle: &SandboxHandle,
        key: &IdempotencyKey,
        request: ExecRequest,
        cancel: &CancelToken,
    ) -> ExecResult {
        cancelled(cancel)?;
        let now = self.shared.clock.now_ms();
        let fp = fingerprint("sim.exec.v1", &request);
        let (ctx, grants) = {
            let st = self.lock()?;
            match Self::exec_gate(&st, handle, key, &fp, now, &request)? {
                Gate::Replay(result) => return result,
                Gate::Proceed { ctx, grants, .. } => (ctx, grants),
            }
        };
        // The values are resolved on this side, for this call, and wiped when it ends. A
        // secret that cannot be resolved stops the call before the marker is written.
        let mut secrets = Vec::with_capacity(grants.len());
        for grant in &grants {
            let value = self
                .shared
                .secrets
                .resolve(&ctx, &grant.secret_ref)
                .await
                .map_err(secret_error)?;
            secrets.push((
                grant.name.clone(),
                Zeroizing::new(value.expose().to_owned()),
            ));
        }
        cancelled(cancel)?;
        let plan = match self.exec_begin(handle, key, &fp, &request, secrets)? {
            Begun::Replay(result) => return result,
            Begun::Run(plan) => *plan,
        };
        let (judged, fs) = self.run(plan);
        self.exec_finish(handle, key, judged, fs)
    }
}

// ---- collect --------------------------------------------------------------------------

impl SimBackend {
    async fn do_collect(
        &self,
        handle: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        cancelled(cancel)?;
        check_key(handle, key, SandboxStep::Collect)?;
        let now = self.shared.clock.now_ms();
        let fp = fingerprint("sim.collect.v1", &req);
        let tenant = handle.tenant_id();
        let mut forge = false;
        let mut fewer = false;
        let (stored, ctx, grants) = {
            let mut st = self.lock()?;
            if let Some(record) = st.record(tenant, key.as_str(), now) {
                if record.sandbox_id != handle.sandbox_id().as_str() {
                    return Err(SandboxError::NotFound);
                }
                if record.fingerprint != fp {
                    return Err(SandboxError::IdempotencyConflict);
                }
                match &record.stored {
                    Stored::Collected(stored) => (
                        Self::copy_stored(stored),
                        stored.ctx.clone(),
                        stored.grants.clone(),
                    ),
                    _ => return Err(SandboxError::Unavailable { retriable: false }),
                }
            } else {
                let sandbox = st.sandbox(handle, now)?;
                if sandbox.phase != Phase::Executed {
                    return Err(SandboxError::InvalidState {
                        from: sandbox.phase.state(),
                    });
                }
                match st.pop_fault(SandboxStep::Collect) {
                    Some(SimFault::Error(error)) => return Err(error),
                    Some(SimFault::ForgedOutput) => forge = true,
                    Some(SimFault::FewerSecrets) => fewer = true,
                    _ => {}
                }
                let sandbox = st.sandbox(handle, now)?;
                let stored = Self::select(sandbox, &req);
                let kept = Self::copy_stored(&stored);
                st.put(
                    tenant,
                    key.as_str(),
                    Record {
                        fingerprint: fp,
                        sandbox_id: handle.sandbox_id().as_str().to_owned(),
                        expires_at: now.saturating_add(self.shared.config.retention_ms),
                        running: false,
                        stored: Stored::Collected(Box::new(stored)),
                    },
                );
                let ctx = kept.ctx.clone();
                let grants = kept.grants.clone();
                (kept, ctx, grants)
            }
        };

        // A delivery that hands back one secret fewer than there are grants.
        let wanted = if fewer {
            grants.len().saturating_sub(1)
        } else {
            grants.len()
        };
        let mut secrets = ResolvedSecrets::new();
        for reference in grants.iter().take(wanted) {
            secrets.push(
                self.shared
                    .secrets
                    .resolve(&ctx, reference)
                    .await
                    .map_err(secret_error)?,
            );
        }
        Ok(Self::deliver(stored, secrets, forge))
    }

    fn copy_stored(stored: &StoredCollected) -> StoredCollected {
        StoredCollected {
            stdout: stored.stdout.clone(),
            stderr: stored.stderr.clone(),
            files: stored.files.clone(),
            truncated: stored.truncated,
            ctx: stored.ctx.clone(),
            grants: stored.grants.clone(),
        }
    }

    /// Pick the files the patterns ask for, within the bounds of the request. Past a bound
    /// the rest is dropped and the delivery is marked truncated.
    fn select(sandbox: &Sandbox, req: &CollectRequest) -> StoredCollected {
        let mut files = Vec::new();
        let mut truncated = !sandbox.truncated.is_empty();
        let mut total = 0u64;
        for (path, bytes) in sandbox.fs.matching(&req.patterns) {
            let size = bytes.len() as u64;
            let Ok(path) = LogicalPath::new(&path) else {
                continue;
            };
            if files.len() as u64 >= u64::from(req.max_files)
                || total.saturating_add(size) > req.max_bytes
            {
                truncated = true;
                break;
            }
            total += size;
            files.push((path, bytes));
        }
        StoredCollected {
            stdout: sandbox.stdout.clone(),
            stderr: sandbox.stderr.clone(),
            files,
            truncated,
            ctx: sandbox.ctx.clone(),
            grants: sandbox
                .grants
                .iter()
                .map(|g| g.secret_ref.clone())
                .collect(),
        }
    }

    /// Build what crosses to the guard. The stored record is honest; a forgery is applied to
    /// the delivery only.
    fn deliver(stored: StoredCollected, secrets: ResolvedSecrets, forge: bool) -> RawCollected {
        let mut raw = RawCollected {
            declared_stdout_sha256: sha256_hex(&stored.stdout),
            declared_stderr_sha256: sha256_hex(&stored.stderr),
            stdout: stored.stdout,
            stderr: stored.stderr,
            files: stored
                .files
                .into_iter()
                .map(|(path, bytes)| RawFile {
                    declared_sha256: sha256_hex(&bytes),
                    path,
                    bytes,
                })
                .collect(),
            declared_truncated: stored.truncated,
            secrets,
        };
        if forge {
            // The digests above were computed over the honest bytes.
            raw.stdout.push(b'!');
        }
        raw
    }
}

// ---- destroy --------------------------------------------------------------------------

impl SimBackend {
    fn do_destroy(&self, handle: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        check_key(handle, key, SandboxStep::Destroy)?;
        let mut st = self.lock()?;
        if let Some(SimFault::Error(error)) = st.pop_fault(SandboxStep::Destroy) {
            return Err(error);
        }
        match st.sandboxes.get(handle.sandbox_id().as_str()) {
            Some(sandbox) if sandbox.tenant != *handle.tenant_id() => Err(SandboxError::NotFound),
            Some(_) => {
                st.sandboxes.remove(handle.sandbox_id().as_str());
                Ok(())
            }
            // Already gone: destroying twice succeeds.
            None => Ok(()),
        }
    }
}

#[async_trait]
impl SandboxBackend for SimBackend {
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        cancelled(cancel)?;
        self.do_create(ctx, key, req)
    }

    async fn exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        self.do_exec(h, key, req, cancel).await
    }

    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        self.do_collect(h, key, req, cancel).await
    }

    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        self.do_destroy(h, key)
    }
}
