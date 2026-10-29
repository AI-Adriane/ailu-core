//! Backends that break one obligation each, built around the simulator. The suite must notice.

use ailu_sandbox::conformance::{
    ConformanceTarget, Hooks, Instance, MarkerState, ProbeKind, ProbeProgram, SimTarget,
};
use ailu_sandbox::sim::{SimBackend, SimConfig};
use ailu_sandbox::{
    CancelToken, CollectRequest, CreateRequest, DeclaredExit, ExecOutcome, ExecRequest, HostRule,
    IdempotencyKey, IsolationDomain, LimitKind, LogicalPath, NetworkPolicy, RawCollected,
    RawCreated, RawFile, ResolvedSecrets, SandboxBackend, SandboxClock, SandboxContext,
    SandboxError, SandboxHandle, SandboxId, SandboxState, SandboxStep, TenantId,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

/// What a mutant gets wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    /// A second exec under another key is accepted.
    SecondExecAllowed,
    /// A second collect under another key is accepted.
    SecondCollectAllowed,
    /// Destroying twice is an error.
    DestroyNotIdempotent,
    /// Destroy answers success and releases nothing.
    DestroyKeepsResources,
    /// The backend never sweeps what is past its ttl.
    NeverSweeps,
    /// The same key with another request gets the first answer instead of a conflict.
    NoConflictCheck,
    /// A replayed exec runs the program again.
    RerunOnReplay,
    /// A replayed exec answers with a result that differs from the first.
    ReplayChangesResult,
    /// No marker is written: after a crash the program is run again.
    NoMarker,
    /// The marker is written but ignored: after a crash the program is run again.
    RerunsAfterCrash,
    /// Answers are forgotten after a day, the policy ceiling is a week.
    ForgetsAfterADay,
    /// A key is forgotten at its sealed deadline itself, not after it.
    ForgetsAtTheDeadline,
    /// A repeated key is served whatever deadline it comes with, later ones included.
    AcceptsLaterDeadlineOnRepeat,
    /// A repeated key is refused when its deadline differs from the sealed one, shorter ones
    /// included.
    RefusesShorterDeadline,
    /// The deduplication store is one table indexed by the text of the key: whoever sends a
    /// key gets what was kept for it, whatever the tenant.
    TenantBlindStore,
    /// A create looks the key up for the tenant the key was derived for, not for the tenant
    /// of the context.
    IgnoresContextTenant,
    /// The backend panics in `create`.
    PanicsOnCreate,
    /// A deadline beyond what the backend keeps is sealed all the same.
    SealsBeyondMaxRetention,
    /// The retention the backend declares shrinks once it has served a request.
    DeclaredRetentionDrifts,
    /// A program that goes past this limit is not killed: the partial outcome is returned as a
    /// success.
    LimitIgnored(LimitKind),
    /// Output past its bound is cut without saying so.
    OutputNotMarked,
    /// The allow list is not applied: every host the simulator knows can be reached.
    AllowListIgnored,
    /// The network is always closed, whatever was asked.
    RefusesEverything,
    /// A program that talks about this host is let through: it connects.
    Reaches(&'static str),
    /// The network probes crash instead of answering reached or refused.
    ProbeCrashes,
    /// The allow list is matched on the host alone: any port of an allowed host is reachable.
    PortIgnored,
    /// Programs start with variables of the host that the profile does not declare.
    InheritsHostEnvironment,
    /// The resolved secrets are not handed back to the guard.
    SecretsNotHandedBack,
    /// A replayed collect hands back no secret: the backend does not resolve them again.
    NoSecretsOnReplay,
    /// A secret reference resolves for every tenant.
    TenantBlindSecrets,
    /// An outgoing link is followed and delivered instead of refused.
    OutgoingLinkFollowed,
    /// A link two directories down is followed: only the direct children are checked.
    DeepLinkFollowed,
    /// An artifact is available to every tenant.
    TenantBlindArtifacts,
    /// The backend never consults the cancel token.
    IgnoresCancel,
    /// The working directory of the request is not used.
    IgnoresCwd,
    /// The input artifacts are not materialized.
    SkipsInputs,
    /// The digest of an exec request leaves out the working directory.
    DigestIgnoresCwd,
    /// The digest of a create request leaves out the secret grants.
    DigestIgnoresEnv,
    /// The isolation domain is derived from the first eight characters of the tenant's name.
    DomainFromTenantPrefix8,
    /// Every sandbox is given the same isolation domain.
    SharedDomain,
    /// The domain of a destroyed sandbox is given to the next tenant that asks.
    RecycledDomain,
    /// Every create returns the id of the first sandbox.
    ReusedSandboxId,
    /// A sandbox is found by its id alone, whoever asks.
    IgnoresTenantOnLookup,
    /// The declared digest of the output is that of the bytes before they were changed.
    StaleDigest,
    /// A file outside the requested patterns is delivered too.
    FileOutsidePatterns,
    /// The same path is delivered twice.
    DuplicatePath,
    /// The delivery goes past every bound of the request.
    PastTheBounds,
    /// The files the program wrote are not delivered.
    DropsFiles,
    /// The standard output is not delivered.
    DropsOutput,
    /// Not a defect: every call gives the executor back once before it starts and once before
    /// it answers. Every case must still pass.
    YieldsOnce,
}

const DAY_MS: u64 = 24 * 3600 * 1000;

/// The simulator with one defect.
pub struct MutantTarget {
    pub base: SimTarget,
    pub mutation: Mutation,
}

impl MutantTarget {
    pub fn new(mutation: Mutation) -> Self {
        let base = match mutation {
            Mutation::InheritsHostEnvironment => SimTarget::with_config(SimConfig {
                base_env: BTreeMap::from([
                    ("LANG".to_owned(), "C".to_owned()),
                    ("PATH".to_owned(), "/usr/bin".to_owned()),
                    ("HOME".to_owned(), "/root".to_owned()),
                    ("CLOUD_CREDENTIAL".to_owned(), "inherited".to_owned()),
                ]),
                ..SimConfig::default()
            }),
            _ => SimTarget::new(),
        };
        Self { base, mutation }
    }
}

#[derive(Default)]
struct Memory {
    destroyed: HashSet<String>,
    /// The first deadline seen with each key.
    sealed: HashMap<String, u64>,
    /// The backend has served a `create`.
    served: bool,
    /// Exec requests seen by key, with the answer.
    execs: HashMap<String, (String, Result<ExecOutcome, SandboxError>)>,
    /// Keys whose exec lost its outcome.
    lost: HashSet<String>,
    /// Programs the mutant "ran" on top of what the simulator ran.
    extra_runs: u64,
    first_seen: HashMap<String, u64>,
    collected: HashSet<String>,
    /// The tenant and domain of each sandbox created.
    domains: HashMap<String, (TenantId, IsolationDomain)>,
    /// The domain of the last sandbox destroyed, with its tenant.
    recycled: Option<(TenantId, IsolationDomain)>,
    first_id: Option<SandboxId>,
    /// Exec requests seen by key under a digest that leaves a field out, with the answer.
    loose_execs: HashMap<String, (String, Result<ExecOutcome, SandboxError>)>,
    loose_creates: HashMap<String, (String, RawCreated)>,
    /// What a store without tenants kept, by the text of the key.
    global_creates: HashMap<String, RawCreated>,
    /// What each sandbox answered to its exec, with the tenant that asked.
    results: HashMap<String, (TenantId, ExecOutcome)>,
}

type Shared = Arc<Mutex<Memory>>;

fn lock(memory: &Shared) -> std::sync::MutexGuard<'_, Memory> {
    memory.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct MutantBackend {
    inner: SimBackend,
    mutation: Mutation,
    memory: Shared,
    clock: Arc<dyn SandboxClock>,
}

impl MutantBackend {
    /// Whether the answer for `key` is past the time this mutant keeps it: a day, or the
    /// deadline it was sealed with, reached.
    fn forgotten(&self, key: &IdempotencyKey, deadline: u64) -> bool {
        let now = self.clock.now_ms();
        match self.mutation {
            Mutation::ForgetsAfterADay => {
                let first = *lock(&self.memory)
                    .first_seen
                    .entry(key.as_str().to_owned())
                    .or_insert(now);
                now.saturating_sub(first) >= DAY_MS
            }
            Mutation::ForgetsAtTheDeadline => {
                let sealed = *lock(&self.memory)
                    .sealed
                    .entry(key.as_str().to_owned())
                    .or_insert(deadline);
                now >= sealed
            }
            _ => false,
        }
    }

    /// Whether this mutant refuses a repeat whose deadline is not the sealed one.
    fn refuses_other_deadline(&self, key: &IdempotencyKey, deadline: u64) -> bool {
        if self.mutation != Mutation::RefusesShorterDeadline {
            return false;
        }
        let sealed = *lock(&self.memory)
            .sealed
            .entry(key.as_str().to_owned())
            .or_insert(deadline);
        deadline != sealed
    }

    /// The deadline this mutant passes on to the simulator: the one it got, or the one it
    /// lets through when it is lax about deadlines.
    fn passed_on(&self, key: &IdempotencyKey, deadline: u64) -> u64 {
        match self.mutation {
            Mutation::AcceptsLaterDeadlineOnRepeat => {
                let sealed = *lock(&self.memory)
                    .sealed
                    .entry(key.as_str().to_owned())
                    .or_insert(deadline);
                deadline.min(sealed)
            }
            Mutation::SealsBeyondMaxRetention => deadline.min(
                self.clock
                    .now_ms()
                    .saturating_add(self.inner.max_retention()),
            ),
            _ => deadline,
        }
    }
}

#[async_trait]
impl ConformanceTarget for MutantTarget {
    type Backend = MutantBackend;

    fn name(&self) -> String {
        format!("mutant {:?}", self.mutation)
    }

    async fn fresh(&self) -> Instance<MutantBackend> {
        let fresh = self.base.fresh().await;
        let memory = Shared::default();
        let hooks = Arc::new(MutantHooks {
            inner: Arc::clone(&fresh.hooks),
            mutation: self.mutation,
            // The simulator is cheap to share: this clone reads the same state.
            sim: fresh.backend.clone(),
            memory: Arc::clone(&memory),
        });
        Instance::new(
            MutantBackend {
                inner: fresh.backend,
                mutation: self.mutation,
                memory,
                clock: Arc::clone(&fresh.clock),
            },
            fresh.clock,
        )
        .with_hooks(hooks)
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        self.base.probe(kind)
    }
}

impl MutantBackend {
    async fn mutant_create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        if self.forgotten(key, deadline) {
            return Err(SandboxError::NotFound);
        }
        if self.mutation == Mutation::PanicsOnCreate {
            panic!("the backend panicked");
        }
        if self.refuses_other_deadline(key, deadline) {
            return Err(SandboxError::Backend("deadline".to_owned()));
        }
        let deadline = self.passed_on(key, deadline);
        if self.mutation == Mutation::TenantBlindStore {
            if let Some(raw) = lock(&self.memory).global_creates.get(key.as_str()) {
                return Ok(raw.clone());
            }
        }
        let mut req = req;
        let loose = (self.mutation == Mutation::DigestIgnoresEnv).then(|| {
            serde_json::to_string(&CreateRequest {
                env: vec![],
                ..req.clone()
            })
            .unwrap_or_default()
        });
        if let Some(loose) = &loose {
            let seen = lock(&self.memory).loose_creates.get(key.as_str()).cloned();
            if let Some((first, raw)) = seen {
                if first == *loose {
                    return Ok(raw);
                }
            }
        }
        match self.mutation {
            Mutation::SkipsInputs => req.inputs.clear(),
            Mutation::AllowListIgnored => req.network = NetworkPolicy::Allow(every_host()),
            Mutation::RefusesEverything => req.network = NetworkPolicy::Deny,
            Mutation::PortIgnored => {
                if let NetworkPolicy::Allow(rules) = &mut req.network {
                    let other_ports: Vec<HostRule> = rules
                        .iter()
                        .map(|rule| HostRule {
                            host: rule.host.clone(),
                            port: 8443,
                        })
                        .collect();
                    rules.extend(other_ports);
                }
            }
            _ => {}
        }
        let never = CancelToken::never();
        let cancel = if self.mutation == Mutation::IgnoresCancel {
            &never
        } else {
            cancel
        };
        let foreign = SandboxContext {
            tenant_id: key.tenant_id().clone(),
            ..ctx.clone()
        };
        let looked_up_for = if self.mutation == Mutation::IgnoresContextTenant {
            &foreign
        } else {
            ctx
        };
        let mut result = self
            .inner
            .create(looked_up_for, key, req, deadline, cancel)
            .await;
        if let (Mutation::TenantBlindStore, Ok(raw)) = (self.mutation, &result) {
            lock(&self.memory)
                .global_creates
                .entry(key.as_str().to_owned())
                .or_insert_with(|| raw.clone());
        }
        if let Ok(raw) = &mut result {
            let mut memory = lock(&self.memory);
            memory.served = true;
            memory.domains.insert(
                raw.sandbox_id.as_str().to_owned(),
                (ctx.tenant_id.clone(), raw.isolation_domain.clone()),
            );
            match self.mutation {
                Mutation::SharedDomain => {
                    raw.isolation_domain = IsolationDomain::new("one-domain-for-all");
                }
                Mutation::DomainFromTenantPrefix8 => {
                    let prefix: String = ctx.tenant_id.as_str().chars().take(8).collect();
                    raw.isolation_domain = IsolationDomain::new(format!("domain-{prefix}"));
                }
                Mutation::RecycledDomain => {
                    if let Some((owner, domain)) = memory.recycled.take() {
                        if owner != ctx.tenant_id {
                            raw.isolation_domain = domain;
                        } else {
                            memory.recycled = Some((owner, domain));
                        }
                    }
                }
                Mutation::ReusedSandboxId => {
                    let first = memory
                        .first_id
                        .get_or_insert_with(|| raw.sandbox_id.clone());
                    raw.sandbox_id = first.clone();
                }
                _ => {}
            }
            if let Some(loose) = loose {
                memory
                    .loose_creates
                    .entry(key.as_str().to_owned())
                    .or_insert((loose, raw.clone()));
            }
        }
        result
    }

    async fn mutant_exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        if self.forgotten(key, deadline) {
            return Err(SandboxError::NotFound);
        }
        if self.refuses_other_deadline(key, deadline) {
            return Err(SandboxError::Backend("deadline".to_owned()));
        }
        let deadline = self.passed_on(key, deadline);
        if self.mutation == Mutation::ProbeCrashes
            && req.argv.first().map(String::as_str) == Some("conf-connect")
        {
            return Ok(ExecOutcome {
                declared_exit: DeclaredExit::Code { code: 2 },
                ..quick_outcome()
            });
        }
        if let Mutation::Reaches(host) = self.mutation {
            if req.argv.iter().any(|arg| arg == host) {
                return Ok(quick_outcome());
            }
        }
        if self.mutation == Mutation::IgnoresTenantOnLookup {
            let answer = lock(&self.memory)
                .results
                .get(h.sandbox_id().as_str())
                .cloned();
            if let Some((owner, outcome)) = answer {
                if owner != *h.tenant_id() {
                    return Ok(outcome);
                }
            }
        }
        let mut req = req;
        if self.mutation == Mutation::IgnoresCwd {
            req.cwd = None;
        }
        let loose = match self.mutation {
            Mutation::DigestIgnoresCwd => Some(ExecRequest {
                cwd: None,
                ..req.clone()
            }),
            _ => None,
        }
        .map(|loose| serde_json::to_string(&loose).unwrap_or_default());
        if let Some(loose) = &loose {
            let seen = lock(&self.memory).loose_execs.get(key.as_str()).cloned();
            if let Some((first, answer)) = seen {
                if first == *loose {
                    return answer;
                }
            }
        }
        let request = serde_json::to_string(&req).unwrap_or_default();
        let seen = lock(&self.memory).execs.get(key.as_str()).cloned();
        if let Some((first_request, answer)) = &seen {
            if self.mutation == Mutation::NoConflictCheck && *first_request != request {
                return answer.clone();
            }
        }
        let never = CancelToken::never();
        let cancel = if self.mutation == Mutation::IgnoresCancel {
            &never
        } else {
            cancel
        };
        let mut result = self.inner.exec(h, key, req, deadline, cancel).await;
        match (self.mutation, &result) {
            (
                Mutation::SecondExecAllowed,
                Err(SandboxError::InvalidState {
                    from: SandboxState::Executed,
                }),
            ) => return Ok(quick_outcome()),
            (
                Mutation::LimitIgnored(ignored),
                Err(SandboxError::LimitExceeded { kind, partial }),
            ) if ignored == *kind => {
                return Ok(ExecOutcome::clone(partial));
            }
            (Mutation::OutputNotMarked, Ok(outcome)) => {
                result = Ok(ExecOutcome {
                    truncated: false,
                    truncation: vec![],
                    ..outcome.clone()
                });
            }
            (Mutation::RerunOnReplay, Ok(_)) if seen.is_some() => {
                lock(&self.memory).extra_runs += 1;
            }
            (Mutation::ReplayChangesResult, Ok(outcome)) if seen.is_some() => {
                result = Ok(ExecOutcome {
                    declared_duration_ms: outcome.declared_duration_ms + 1,
                    ..outcome.clone()
                });
            }
            (
                Mutation::NoMarker | Mutation::RerunsAfterCrash,
                Err(SandboxError::Unavailable { retriable: true }),
            ) => {
                lock(&self.memory).lost.insert(key.as_str().to_owned());
            }
            (
                Mutation::NoMarker | Mutation::RerunsAfterCrash,
                Err(SandboxError::Unavailable { retriable: false }),
            ) if lock(&self.memory).lost.contains(key.as_str()) => {
                // Nothing says the program ran: it runs again.
                lock(&self.memory).extra_runs += 1;
                return Ok(quick_outcome());
            }
            _ => {}
        }
        if let Ok(outcome) = &result {
            lock(&self.memory).results.insert(
                h.sandbox_id().as_str().to_owned(),
                (h.tenant_id().clone(), outcome.clone()),
            );
        }
        if let (Some(loose), true) = (loose, result.is_ok()) {
            lock(&self.memory)
                .loose_execs
                .entry(key.as_str().to_owned())
                .or_insert((loose, result.clone()));
        }
        if seen.is_none() && result.is_ok() {
            lock(&self.memory)
                .execs
                .insert(key.as_str().to_owned(), (request, result.clone()));
        }
        result
    }

    async fn mutant_collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        deadline: u64,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        if self.forgotten(key, deadline) {
            return Err(SandboxError::NotFound);
        }
        if self.refuses_other_deadline(key, deadline) {
            return Err(SandboxError::Backend("deadline".to_owned()));
        }
        let deadline = self.passed_on(key, deadline);
        let deep = req.patterns.iter().any(|pattern| pattern.contains("**"));
        let never = CancelToken::never();
        let cancel = if self.mutation == Mutation::IgnoresCancel {
            &never
        } else {
            cancel
        };
        let mut result = self.inner.collect(h, key, req, deadline, cancel).await;
        match (self.mutation, &mut result) {
            (
                Mutation::SecondCollectAllowed,
                Err(SandboxError::InvalidState {
                    from: SandboxState::Collected,
                }),
            ) => Ok(RawCollected::default()),
            (Mutation::OutgoingLinkFollowed, Err(SandboxError::InvalidPath { .. })) => {
                Ok(RawCollected::default())
            }
            (Mutation::DeepLinkFollowed, Err(SandboxError::InvalidPath { .. })) if deep => {
                Ok(RawCollected::default())
            }
            (Mutation::SecretsNotHandedBack, Ok(raw)) => {
                raw.secrets = ResolvedSecrets::new();
                result
            }
            (Mutation::NoSecretsOnReplay, Ok(raw)) => {
                let replayed = !lock(&self.memory).collected.insert(key.as_str().to_owned());
                if replayed {
                    raw.secrets = ResolvedSecrets::new();
                }
                result
            }
            (Mutation::OutputNotMarked, Ok(raw)) => {
                raw.declared_truncated = false;
                result
            }
            (Mutation::StaleDigest, Ok(raw)) => {
                raw.stdout.push(b'!');
                result
            }
            (Mutation::FileOutsidePatterns, Ok(raw)) => {
                raw.files.extend(file("never-requested/anything", b"y"));
                result
            }
            (Mutation::DuplicatePath, Ok(raw)) => {
                raw.files.extend(file("out/twice.txt", b"one"));
                raw.files.extend(file("out/twice.txt", b"two"));
                result
            }
            (Mutation::DropsFiles, Ok(raw)) => {
                raw.files.clear();
                result
            }
            (Mutation::DropsOutput, Ok(raw)) => {
                raw.stdout.clear();
                raw.declared_stdout_sha256 = digest(&[]);
                result
            }
            (Mutation::PastTheBounds, Ok(raw)) => {
                raw.stdout = vec![b'x'; 100];
                raw.declared_stdout_sha256 = digest(&raw.stdout);
                raw.files.extend(file("out/over1", b"zz"));
                raw.files.extend(file("out/over2", b"zz"));
                result
            }
            _ => result,
        }
    }

    async fn mutant_destroy(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
    ) -> Result<(), SandboxError> {
        if self.mutation == Mutation::RecycledDomain {
            let mut memory = lock(&self.memory);
            if let Some(owned) = memory.domains.get(h.sandbox_id().as_str()).cloned() {
                memory.recycled = Some(owned);
            }
        }
        match self.mutation {
            Mutation::DestroyKeepsResources => Ok(()),
            Mutation::DestroyNotIdempotent => {
                let first = lock(&self.memory)
                    .destroyed
                    .insert(h.sandbox_id().as_str().to_owned());
                if first {
                    self.inner.destroy(h, key).await
                } else {
                    Err(SandboxError::NotFound)
                }
            }
            _ => self.inner.destroy(h, key).await,
        }
    }
}

impl MutantBackend {
    /// A backend that gives the executor back once before it answers (and before it starts).
    async fn pause(&self) {
        if self.mutation == Mutation::YieldsOnce {
            let mut yielded = false;
            std::future::poll_fn(|cx| {
                if yielded {
                    std::task::Poll::Ready(())
                } else {
                    yielded = true;
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
        }
    }
}

#[async_trait]
impl SandboxBackend for MutantBackend {
    fn max_retention(&self) -> u64 {
        let declared = self.inner.max_retention();
        if self.mutation == Mutation::DeclaredRetentionDrifts && lock(&self.memory).served {
            return declared / 2;
        }
        declared
    }

    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        attempt_deadline_ms: u64,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        self.pause().await;
        let result = self
            .mutant_create(ctx, key, req, attempt_deadline_ms, cancel)
            .await;
        self.pause().await;
        result
    }

    async fn exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        attempt_deadline_ms: u64,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        self.pause().await;
        let result = self
            .mutant_exec(h, key, req, attempt_deadline_ms, cancel)
            .await;
        self.pause().await;
        result
    }

    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        attempt_deadline_ms: u64,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        self.pause().await;
        let result = self
            .mutant_collect(h, key, req, attempt_deadline_ms, cancel)
            .await;
        self.pause().await;
        result
    }

    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        self.pause().await;
        let result = self.mutant_destroy(h, key).await;
        self.pause().await;
        result
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file(path: &str, bytes: &[u8]) -> Option<RawFile> {
    Some(RawFile {
        path: LogicalPath::new(path).ok()?,
        bytes: bytes.to_vec(),
        declared_sha256: digest(bytes),
    })
}

/// Every host the simulator knows, listed or not.
fn every_host() -> Vec<HostRule> {
    let mut rules = SimTarget::reference_rules();
    rules.push(HostRule {
        host: "other.example.com".to_owned(),
        port: 443,
    });
    rules
}

fn quick_outcome() -> ExecOutcome {
    ExecOutcome {
        declared_exit: DeclaredExit::Code { code: 0 },
        truncated: false,
        truncation: vec![],
        declared_duration_ms: 1,
    }
}

struct MutantHooks {
    inner: Arc<dyn Hooks>,
    mutation: Mutation,
    sim: SimBackend,
    memory: Shared,
}

#[async_trait]
impl Hooks for MutantHooks {
    async fn advance_clock(&self, ms: u64) -> bool {
        self.inner.advance_clock(ms).await
    }

    async fn residue_after_sweep(&self) -> Option<usize> {
        if self.mutation == Mutation::NeverSweeps {
            // No sweep: what is past its ttl is still there.
            return Some(self.sim.residue());
        }
        self.inner.residue_after_sweep().await
    }

    async fn live_resources(&self) -> Option<usize> {
        self.inner.live_resources().await
    }

    async fn executions(&self) -> Option<u64> {
        let extra = lock(&self.memory).extra_runs;
        self.inner.executions().await.map(|runs| runs + extra)
    }

    async fn arm_crash_after_marker(&self) -> bool {
        self.inner.arm_crash_after_marker().await
    }

    async fn marker(&self, ctx: &SandboxContext, step: SandboxStep) -> Option<MarkerState> {
        let kept = self.inner.marker(ctx, step).await;
        if self.mutation == Mutation::NoMarker && kept == Some(MarkerState::MarkerOnly) {
            return Some(MarkerState::Absent);
        }
        kept
    }

    async fn retention_ms(&self) -> Option<u64> {
        self.inner.retention_ms().await
    }

    async fn provide_secret(&self, tenant: &TenantId, reference: &str, value: &str) -> bool {
        if self.mutation == Mutation::TenantBlindSecrets {
            // The store does not tell tenants apart.
            for other in ["tenant-a", "tenant-b"] {
                self.inner
                    .provide_secret(&TenantId::new(other), reference, value)
                    .await;
            }
        }
        self.inner.provide_secret(tenant, reference, value).await
    }

    async fn provide_artifact(
        &self,
        tenant: &TenantId,
        artifact: &ailu_fs_backend::ArtifactRef,
        bytes: &[u8],
    ) -> bool {
        if self.mutation == Mutation::TenantBlindArtifacts {
            // The store does not tell tenants apart.
            for other in ["tenant-a", "tenant-b"] {
                self.inner
                    .provide_artifact(&TenantId::new(other), artifact, bytes)
                    .await;
            }
        }
        self.inner.provide_artifact(tenant, artifact, bytes).await
    }

    async fn base_environment(&self) -> Option<Vec<String>> {
        if self.mutation == Mutation::InheritsHostEnvironment {
            // What the profile declares, not what the programs get.
            return Some(vec!["LANG".to_owned(), "PATH".to_owned()]);
        }
        self.inner.base_environment().await
    }
}
