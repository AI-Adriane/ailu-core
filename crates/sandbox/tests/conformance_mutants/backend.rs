//! Backends that break one obligation each, built around the simulator. The suite must notice.

use ailu_sandbox::conformance::{
    ConformanceTarget, Hooks, Instance, MarkerState, ProbeKind, ProbeProgram, SimTarget,
};
use ailu_sandbox::sim::SimBackend;
use ailu_sandbox::{
    CancelToken, CollectRequest, CreateRequest, DeclaredExit, ExecOutcome, ExecRequest,
    IdempotencyKey, LimitKind, RawCollected, RawCreated, SandboxBackend, SandboxClock,
    SandboxContext, SandboxError, SandboxHandle, SandboxState, SandboxStep, TenantId,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
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
    /// A program that goes past this limit is not killed: the partial outcome is returned as a
    /// success.
    LimitIgnored(LimitKind),
    /// Output past its bound is cut without saying so.
    OutputNotMarked,
}

const DAY_MS: u64 = 24 * 3600 * 1000;

/// The simulator with one defect.
pub struct MutantTarget {
    pub base: SimTarget,
    pub mutation: Mutation,
}

impl MutantTarget {
    pub fn new(mutation: Mutation) -> Self {
        Self {
            base: SimTarget::new(),
            mutation,
        }
    }
}

#[derive(Default)]
struct Memory {
    destroyed: HashSet<String>,
    /// Exec requests seen by key, with the answer.
    execs: HashMap<String, (String, Result<ExecOutcome, SandboxError>)>,
    /// Keys whose exec lost its outcome.
    lost: HashSet<String>,
    /// Programs the mutant "ran" on top of what the simulator ran.
    extra_runs: u64,
    first_seen: HashMap<String, u64>,
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
    /// Whether the answer for `key` is past the day this mutant keeps it.
    fn forgotten(&self, key: &IdempotencyKey) -> bool {
        if self.mutation != Mutation::ForgetsAfterADay {
            return false;
        }
        let now = self.clock.now_ms();
        let first = *lock(&self.memory)
            .first_seen
            .entry(key.as_str().to_owned())
            .or_insert(now);
        now.saturating_sub(first) >= DAY_MS
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

#[async_trait]
impl SandboxBackend for MutantBackend {
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        if self.forgotten(key) {
            return Err(SandboxError::NotFound);
        }
        self.inner.create(ctx, key, req, cancel).await
    }

    async fn exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        if self.forgotten(key) {
            return Err(SandboxError::NotFound);
        }
        let request = serde_json::to_string(&req).unwrap_or_default();
        let seen = lock(&self.memory).execs.get(key.as_str()).cloned();
        if let Some((first_request, answer)) = &seen {
            if self.mutation == Mutation::NoConflictCheck && *first_request != request {
                return answer.clone();
            }
        }
        let mut result = self.inner.exec(h, key, req, cancel).await;
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
        if seen.is_none() && result.is_ok() {
            lock(&self.memory)
                .execs
                .insert(key.as_str().to_owned(), (request, result.clone()));
        }
        result
    }

    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        if self.forgotten(key) {
            return Err(SandboxError::NotFound);
        }
        let mut result = self.inner.collect(h, key, req, cancel).await;
        match (self.mutation, &mut result) {
            (
                Mutation::SecondCollectAllowed,
                Err(SandboxError::InvalidState {
                    from: SandboxState::Collected,
                }),
            ) => Ok(RawCollected::default()),
            (Mutation::OutputNotMarked, Ok(raw)) => {
                raw.declared_truncated = false;
                result
            }
            _ => result,
        }
    }

    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
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
        self.inner.provide_secret(tenant, reference, value).await
    }

    async fn provide_artifact(
        &self,
        tenant: &TenantId,
        artifact: &ailu_fs_backend::ArtifactRef,
        bytes: &[u8],
    ) -> bool {
        self.inner.provide_artifact(tenant, artifact, bytes).await
    }

    async fn base_environment(&self) -> Option<Vec<String>> {
        self.inner.base_environment().await
    }
}
