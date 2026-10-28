//! The environment of one case: a guard over a tap over a fresh instance of the target, with
//! a journal and a verifier the suite controls, and the requests the cases build on.

use super::tap::{Tap, TestJournal};
use super::{skip, Must, Stop};
use crate::approval::{GrantVerifier, InMemoryGrantVerifier, VerifiedGrant};
use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::clock::SandboxClock;
use crate::collected::{Collected, RawCollected};
use crate::conformance::target::{ConformanceTarget, Hooks, ProbeKind, ProbeProgram};
use crate::error::SandboxError;
use crate::guard::GovernedSandbox;
use crate::ids::{
    IdempotencyKey, RawCreated, SandboxContext, SandboxHandle, SandboxStep, TenantId,
};
use crate::journal::{SandboxEventKind, SandboxJournal};
use crate::limits::Limits;
use crate::outcome::ExecOutcome;
use crate::policy::SandboxPolicy;
use crate::request::{CollectRequest, CreateRequest, ExecRequest, NetworkPolicy};
use crate::ApprovalClaim;
use ailu_graph_core::{NodeId, RunId};
use async_trait::async_trait;
use std::sync::Arc;

/// A target with its generic parts erased, so that the cases are plain functions.
#[async_trait]
pub(crate) trait Source: Send + Sync {
    async fn parts(&self) -> Parts;
    fn probe_for(&self, kind: &ProbeKind) -> Option<ProbeProgram>;
}

pub(crate) struct Parts {
    backend: Arc<dyn SandboxBackend>,
    clock: Arc<dyn SandboxClock>,
    hooks: Arc<dyn Hooks>,
    profile: String,
}

#[async_trait]
impl<T: ConformanceTarget> Source for T {
    async fn parts(&self) -> Parts {
        let instance = self.fresh().await;
        Parts {
            backend: Arc::new(instance.backend),
            clock: instance.clock,
            hooks: instance.hooks,
            profile: self.profile(),
        }
    }

    fn probe_for(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        ConformanceTarget::probe(self, kind)
    }
}

/// What a case is given: a way to get fresh environments and the target's probes.
pub(crate) struct Lab<'t> {
    pub(crate) source: &'t dyn Source,
}

impl Lab<'_> {
    /// A fresh environment under the default policy.
    pub(crate) async fn env(&self) -> Env {
        self.env_with(SandboxPolicy::default()).await
    }

    /// A fresh environment under `policy`.
    pub(crate) async fn env_with(&self, policy: SandboxPolicy) -> Env {
        Env::build(self.source.parts().await, policy)
    }

    /// The command for `kind`; the case is not applicable when the target has none.
    pub(crate) fn probe(&self, kind: &ProbeKind) -> Result<ProbeProgram, Stop> {
        match self.source.probe_for(kind) {
            Some(program) => Ok(program),
            None => skip!("the target offers no probe for {kind:?}"),
        }
    }
}

/// One case's environment.
pub(crate) struct Env {
    pub(crate) tap: Tap,
    pub(crate) guard: GovernedSandbox<Tap>,
    pub(crate) journal: Arc<TestJournal>,
    pub(crate) verifier: Arc<InMemoryGrantVerifier>,
    pub(crate) clock: Arc<dyn SandboxClock>,
    pub(crate) hooks: Arc<dyn Hooks>,
    pub(crate) policy: SandboxPolicy,
    profile: String,
}

/// Run id of every context the suite builds.
const RUN: &str = "conformance-run";

impl Env {
    fn build(parts: Parts, policy: SandboxPolicy) -> Self {
        let tap = Tap::new(parts.backend);
        let journal = Arc::new(TestJournal::default());
        let verifier = Arc::new(InMemoryGrantVerifier::new());
        let guard = GovernedSandbox::new(
            tap.clone(),
            policy.clone(),
            Arc::clone(&parts.clock),
            Arc::clone(&journal) as Arc<dyn SandboxJournal>,
            Arc::clone(&verifier) as Arc<dyn GrantVerifier>,
        )
        .expect("the suite builds only valid policies");
        Self {
            tap,
            guard,
            journal,
            verifier,
            clock: parts.clock,
            hooks: parts.hooks,
            policy,
            profile: parts.profile,
        }
    }

    /// A second guard over the same backend, journal and verifier: the host restarted and
    /// replays the node.
    pub(crate) fn restarted(&self) -> GovernedSandbox<Tap> {
        self.restarted_under(self.policy.clone())
    }

    /// Like [`Env::restarted`], under another policy.
    pub(crate) fn restarted_under(&self, policy: SandboxPolicy) -> GovernedSandbox<Tap> {
        GovernedSandbox::new(
            self.tap.clone(),
            policy,
            Arc::clone(&self.clock),
            Arc::clone(&self.journal) as Arc<dyn SandboxJournal>,
            Arc::clone(&self.verifier) as Arc<dyn GrantVerifier>,
        )
        .expect("the suite builds only valid policies")
    }

    // ---- requests -----------------------------------------------------------------------

    pub(crate) fn ctx(&self, tenant: &str, node: &str) -> SandboxContext {
        SandboxContext {
            tenant_id: TenantId::new(tenant),
            run_id: RunId::from(RUN),
            node_id: NodeId::from(node),
            attempt: 1,
        }
    }

    pub(crate) fn create_req(&self) -> CreateRequest {
        CreateRequest {
            profile: self.profile.clone(),
            limits: Limits::default(),
            network: NetworkPolicy::Deny,
            inputs: vec![],
            env: vec![],
            ttl_ms: 60_000,
        }
    }

    pub(crate) fn collect_req(&self) -> CollectRequest {
        CollectRequest {
            patterns: vec!["out/*".to_owned()],
            max_files: 8,
            max_bytes: 64 * 1024,
        }
    }

    pub(crate) fn exec_of(program: &ProbeProgram) -> ExecRequest {
        ExecRequest {
            argv: program.argv.clone(),
            cwd: None,
            stdin: None,
        }
    }

    /// The instant the run is said to have started: a little before now.
    pub(crate) fn run_started(&self) -> u64 {
        self.clock.now_ms().saturating_sub(1_000)
    }

    pub(crate) fn never() -> CancelToken {
        CancelToken::never()
    }

    // ---- approval -----------------------------------------------------------------------

    /// Record an approval by someone other than the requester for exactly this call, and
    /// return the claim a node would present.
    pub(crate) fn approve(
        &self,
        ctx: &SandboxContext,
        create: &CreateRequest,
        exec: &ExecRequest,
    ) -> Result<ApprovalClaim, SandboxError> {
        self.approve_by(ctx, create, exec, "agent", "reviewer", &"ab".repeat(32))
    }

    pub(crate) fn approve_by(
        &self,
        ctx: &SandboxContext,
        create: &CreateRequest,
        exec: &ExecRequest,
        requested_by: &str,
        approved_by: &str,
        payload_hash: &str,
    ) -> Result<ApprovalClaim, SandboxError> {
        let request = self.guard.request_approval(ctx, create, exec)?;
        let approval_id = format!("apr-{}-{}", approved_by, &request.call_key[13..29]);
        self.verifier.approve(VerifiedGrant {
            approval_id: approval_id.clone(),
            call_key: request.call_key.clone(),
            tenant_id: ctx.tenant_id.clone(),
            run_id: ctx.run_id.clone(),
            requested_by: requested_by.to_owned(),
            approved_by: approved_by.to_owned(),
            approval_payload_hash: payload_hash.to_owned(),
        });
        Ok(ApprovalClaim {
            approval_id,
            call_key: request.call_key,
        })
    }

    // ---- the calls ----------------------------------------------------------------------

    /// Approve and create.
    pub(crate) async fn ready(
        &self,
        ctx: &SandboxContext,
        create: CreateRequest,
        exec: &ExecRequest,
    ) -> Result<(SandboxHandle, ApprovalClaim), Stop> {
        let claim = self.approve(ctx, &create, exec).must("approval request")?;
        let handle = self
            .guard
            .create(
                ctx,
                create,
                exec,
                Some(&claim),
                self.run_started(),
                &Self::never(),
            )
            .await
            .must("create")?;
        Ok((handle, claim))
    }

    pub(crate) async fn run(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        exec: ExecRequest,
        claim: &ApprovalClaim,
    ) -> Result<ExecOutcome, SandboxError> {
        self.guard
            .exec(
                ctx,
                handle,
                exec,
                Some(claim),
                self.run_started(),
                &Self::never(),
            )
            .await
    }

    pub(crate) async fn gather(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<Collected, SandboxError> {
        self.guard
            .collect(
                ctx,
                handle,
                self.collect_req(),
                self.run_started(),
                &Self::never(),
            )
            .await
    }

    /// Create, run and collect one program under `create`; the sandbox is left in place.
    pub(crate) async fn cycle(
        &self,
        ctx: &SandboxContext,
        create: CreateRequest,
        exec: &ExecRequest,
    ) -> Result<Ran, Stop> {
        let (handle, claim) = self.ready(ctx, create, exec).await?;
        let outcome = self
            .run(ctx, &handle, exec.clone(), &claim)
            .await
            .must("exec")?;
        let collected = self.gather(ctx, &handle).await.must("collect")?;
        Ok(Ran {
            handle,
            claim,
            outcome,
            collected,
        })
    }

    /// The node, run again through `guard` (a restarted host's): create, exec, collect.
    pub(crate) async fn replay(
        &self,
        guard: &GovernedSandbox<Tap>,
        ctx: &SandboxContext,
        create: CreateRequest,
        exec: &ExecRequest,
        claim: &ApprovalClaim,
    ) -> Result<(ExecOutcome, Collected), Stop> {
        let start = self.run_started();
        let never = Self::never();
        let handle = guard
            .create(ctx, create, exec, Some(claim), start, &never)
            .await
            .must("replayed create")?;
        let outcome = guard
            .exec(ctx, &handle, exec.clone(), Some(claim), start, &never)
            .await
            .must("replayed exec")?;
        let collected = guard
            .collect(ctx, &handle, self.collect_req(), start, &never)
            .await
            .must("replayed collect")?;
        Ok((outcome, collected))
    }

    // ---- the backend, without the guard ---------------------------------------------------

    /// The key of `step` of `ctx`, as the guard would derive it.
    pub(crate) fn key(ctx: &SandboxContext, step: SandboxStep) -> IdempotencyKey {
        IdempotencyKey::new(ctx, step)
    }

    /// The same node on its next attempt: another set of keys.
    pub(crate) fn next_attempt(ctx: &SandboxContext) -> SandboxContext {
        SandboxContext {
            attempt: ctx.attempt + 1,
            ..ctx.clone()
        }
    }

    pub(crate) async fn b_create(
        &self,
        ctx: &SandboxContext,
        req: CreateRequest,
    ) -> Result<(SandboxHandle, RawCreated), SandboxError> {
        let key = Self::key(ctx, SandboxStep::Create);
        let raw = self.tap.create(ctx, &key, req, &Self::never()).await?;
        Ok((SandboxHandle::from_created(&raw, &ctx.tenant_id), raw))
    }

    pub(crate) async fn b_exec(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: ExecRequest,
    ) -> Result<ExecOutcome, SandboxError> {
        let key = Self::key(ctx, SandboxStep::Exec);
        self.tap.exec(handle, &key, req, &Self::never()).await
    }

    pub(crate) async fn b_collect(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<RawCollected, SandboxError> {
        self.b_collect_with(ctx, handle, self.collect_req()).await
    }

    pub(crate) async fn b_collect_with(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
        req: CollectRequest,
    ) -> Result<RawCollected, SandboxError> {
        let key = Self::key(ctx, SandboxStep::Collect);
        self.tap.collect(handle, &key, req, &Self::never()).await
    }

    pub(crate) async fn b_destroy(
        &self,
        ctx: &SandboxContext,
        handle: &SandboxHandle,
    ) -> Result<(), SandboxError> {
        let key = Self::key(ctx, SandboxStep::Destroy);
        self.tap.destroy(handle, &key).await
    }

    /// The kinds of the events in the guard's journal, in order.
    pub(crate) fn kinds(&self) -> Vec<SandboxEventKind> {
        self.journal
            .events()
            .iter()
            .map(|event| event.kind)
            .collect()
    }

    /// The `Success` command as a plain request, or not applicable.
    pub(crate) fn success(lab: &Lab<'_>) -> Result<ExecRequest, Stop> {
        Ok(Self::exec_of(&lab.probe(&ProbeKind::Success)?))
    }
}

/// What a full cycle produced.
pub(crate) struct Ran {
    pub(crate) handle: SandboxHandle,
    pub(crate) claim: ApprovalClaim,
    pub(crate) outcome: ExecOutcome,
    pub(crate) collected: Collected,
}
