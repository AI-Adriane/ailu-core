//! The guard with a proof signer: what it signs, in which order, and what it does when the
//! signer fails.

use super::*;
use crate::proof::{
    verify_chain, verify_proof, InProcessProofSigner, ProofSigner, SignedExecutionProof,
    SignerError, SignerRefusal, TrustedKeys,
};
use crate::ProvedCollected;
use std::sync::atomic::AtomicUsize;

enum Script {
    Fail(SignerError),
    Rewrite(Box<dyn FnOnce(SignedExecutionProof) -> SignedExecutionProof + Send>),
}

/// A real in-process signer that a test can make fail or lie, once.
struct ScriptedSigner {
    inner: InProcessProofSigner,
    script: Mutex<VecDeque<Script>>,
    calls: AtomicUsize,
}

impl ScriptedSigner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InProcessProofSigner::from_seed("k1", &[7; 32]),
            script: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
        })
    }

    fn fail_next(&self, error: SignerError) {
        self.script.lock().unwrap().push_back(Script::Fail(error));
    }

    fn rewrite_next(
        &self,
        f: impl FnOnce(SignedExecutionProof) -> SignedExecutionProof + Send + 'static,
    ) {
        self.script
            .lock()
            .unwrap()
            .push_back(Script::Rewrite(Box::new(f)));
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn keys(&self) -> TrustedKeys {
        let mut keys = TrustedKeys::new();
        keys.trust("k1", &self.inner.public_key()).unwrap();
        keys
    }
}

#[async_trait]
impl ProofSigner for ScriptedSigner {
    async fn sign(
        &self,
        proof: &crate::proof::ExecutionProof,
    ) -> Result<SignedExecutionProof, SignerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.script.lock().unwrap().pop_front();
        if let Some(Script::Fail(error)) = next {
            return Err(error);
        }
        let signed = self.inner.sign(proof).await?;
        Ok(match next {
            Some(Script::Rewrite(f)) => f(signed),
            _ => signed,
        })
    }
}

fn signed_fx() -> (Fx, Arc<ScriptedSigner>) {
    let signer = ScriptedSigner::new();
    let Fx {
        guard,
        spy,
        journal,
        clock,
        verifier,
        verify_advance,
    } = fx();
    let guard = guard.with_proof_signer(Arc::clone(&signer) as Arc<dyn ProofSigner>);
    (
        Fx {
            guard,
            spy,
            journal,
            clock,
            verifier,
            verify_advance,
        },
        signer,
    )
}

/// A restarted host: a new guard over the same backend, journal, verifier and signer.
fn restarted_signed(fx: &Fx, signer: &Arc<ScriptedSigner>) -> GovernedSandbox<Shared> {
    restarted(fx).with_proof_signer(Arc::clone(signer) as Arc<dyn ProofSigner>)
}

async fn gather(
    fx: &Fx,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
) -> Result<ProvedCollected, SandboxError> {
    fx.guard
        .collect_proved(ctx, handle, collect_req(), RUN_STARTED, &never())
        .await
}

/// Approve, create and run the sandbox of `ctx`.
async fn executed(fx: &Fx, ctx: &SandboxContext) -> SandboxHandle {
    let (handle, grant) = ready(fx, ctx).await;
    run(fx, ctx, &handle, &grant).await.unwrap();
    handle
}

fn sha(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

#[tokio::test]
async fn without_a_signer_nothing_changes_and_a_proof_cannot_be_asked_for() {
    let fx = fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    // No proof to give: refused before anything is done, and not retriable.
    let result = fx
        .guard
        .collect_proved(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        result.unwrap_err(),
        SandboxError::Unavailable { retriable: false }
    );
    assert_eq!(fx.spy.count(SandboxStep::Collect), 0);
    // The plain collect is what it always was.
    let collected = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(collected.stdout, b"hello");
    assert!(fx.kinds().contains(&SandboxEventKind::SandboxCollected));
}

#[tokio::test]
async fn the_proof_states_what_the_guard_recomputed_and_what_was_approved() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    // The clock moves while the program runs: the proof dates the exec with the guard's clock.
    let clock = Arc::clone(&fx.clock);
    *fx.spy.exec_hook.lock().unwrap() = Some(Box::new(move || {
        clock.0.store(NOW + 40, Ordering::SeqCst);
    }));
    fx.clock.0.store(NOW + 10, Ordering::SeqCst);
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.clock.0.store(NOW + 900, Ordering::SeqCst);

    let ProvedCollected { collected, proof } = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(verify_proof(&proof, &signer.keys()), Ok(()));
    let p = &proof.proof;
    assert_eq!(p.kind, "sandbox_execution");
    assert_eq!(
        (p.tenant_id.as_str(), p.run_id.as_str(), p.node_id.as_str()),
        ("tenant-a", "run-1", "node-1")
    );
    assert_eq!(p.sandbox_id, handle.sandbox_id().as_str());
    assert_eq!(p.isolation_domain, "dom-tenant-a");
    assert_eq!(
        p.idempotency_key,
        IdempotencyKey::new(&ctx, SandboxStep::Collect).as_str()
    );
    assert_eq!(p.stdout_digest, collected.stdout_sha256);
    assert_eq!(p.stdout_digest, sha("hello"));
    assert_eq!(p.stderr_digest, collected.stderr_sha256);
    assert_eq!(p.outputs_digest, collected.outputs_digest);
    assert_eq!(p.profile_digest, sha("default"));
    assert_eq!(p.limits_effective, Limits::default());
    assert_eq!(p.declared_exit, DeclaredExit::Code { code: 0 });
    assert_eq!(p.declared_duration_ms, 3);
    assert!(!p.truncated && !p.violation);
    // Dated by the guard's clock: sent at +10, answered at +40 (the hook moved it).
    assert_eq!((p.started_at, p.finished_at), (NOW + 10, NOW + 40));
    // The approval is the one `exec` was let through with, remembered for `collect`.
    assert_eq!(p.approval.call_key, grant.call_key);
    assert_eq!(p.approval.approved_by, "alice");
    assert_eq!(p.approval_payload_hash, "ab".repeat(32));
    assert_eq!(proof.prev_hash, None);
    // Digests only: nothing of the run's content is in the record.
    let text = serde_json::to_string(&proof).unwrap();
    for content in ["hello", "s3cr3t-arg", "tool"] {
        assert!(!text.contains(content), "{content} is in the proof");
    }
}

#[tokio::test]
async fn the_digests_are_those_of_the_delivered_redacted_bytes_not_the_raw_ones() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let mut create = create_req();
    create.env = vec![EnvGrant {
        name: "TOKEN".to_owned(),
        secret_ref: SecretRef("vault/t".to_owned()),
    }];
    let grant = grant_for(&fx, &ctx, &create, &exec_req());
    let handle = fx
        .guard
        .create(
            &ctx,
            create,
            &exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let mut raw = raw_collected(b"token=s3cr3tvalue", b"");
    raw.secrets.push(ResolvedSecret::new("s3cr3tvalue"));
    fx.spy.script_collect(Ok(raw));
    let ProvedCollected { collected, proof } = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(collected.stdout, b"token=[REDACTED]");
    assert_eq!(proof.proof.stdout_digest, sha("token=[REDACTED]"));
    assert_ne!(proof.proof.stdout_digest, sha("token=s3cr3tvalue"));
    assert_eq!(verify_proof(&proof, &signer.keys()), Ok(()));
}

#[tokio::test]
async fn a_signer_that_is_unavailable_fails_the_node_closed_and_the_sandbox_stays_collectable() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    signer.fail_next(SignerError::Unavailable);
    let error = gather(&fx, &ctx, &handle).await.unwrap_err();
    assert_eq!(error, SandboxError::Unavailable { retriable: true });
    assert_eq!(error.code(), "unavailable");
    // No result was recorded or announced; the program ran once.
    assert!(!fx.kinds().contains(&SandboxEventKind::SandboxCollected));
    assert_eq!(fx.spy.count(SandboxStep::Exec), 1);
    // Asked again, the same sandbox is collected and signed.
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
    assert_eq!(
        fx.spy.count(SandboxStep::Exec),
        1,
        "the effect is not repeated"
    );
    assert_eq!(
        fx.spy.count(SandboxStep::Collect),
        2,
        "nothing was recorded by the failure"
    );
    assert!(fx.kinds().contains(&SandboxEventKind::SandboxCollected));
}

#[tokio::test]
async fn a_signer_that_refuses_gives_a_dedicated_code_and_is_journaled() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    signer.fail_next(SignerError::Refused(SignerRefusal::Rejected));
    let error = gather(&fx, &ctx, &handle).await.unwrap_err();
    assert_eq!(error.full_code(), "policy_denied.proof_refused");
    assert_eq!(error.code(), "policy_denied");
    assert!(!error.is_retriable());
    let denied: Vec<_> = fx
        .journal
        .inner
        .events()
        .into_iter()
        .filter(|e| e.kind == SandboxEventKind::SandboxDenied)
        .collect();
    assert_eq!(denied.len(), 1);
    assert_eq!(
        denied[0].code.as_deref(),
        Some("policy_denied.proof_refused")
    );
    assert!(!fx.kinds().contains(&SandboxEventKind::SandboxCollected));
    assert!(
        gather(&fx, &ctx, &handle).await.is_ok(),
        "collectable again"
    );
}

#[tokio::test]
async fn a_record_that_is_not_about_the_submitted_proof_is_refused() {
    type Tamper = Box<dyn FnOnce(SignedExecutionProof) -> SignedExecutionProof + Send>;
    let tampers: Vec<(&str, Tamper)> = vec![
        (
            "another digest",
            Box::new(|mut r| {
                r.proof.stdout_digest = sha("forged");
                r
            }),
        ),
        (
            "another kind",
            Box::new(|mut r| {
                r.proof.kind = "approval".to_owned();
                r
            }),
        ),
        (
            "a coherent record, re-hashed, for another proof",
            Box::new(|mut r| {
                r.proof.stdout_digest = sha("forged");
                r.payload_hash = crate::proof::payload_hash(&r.proof).unwrap();
                r
            }),
        ),
        (
            "a coherent record for another approval",
            Box::new(|mut r| {
                r.proof.approval_payload_hash = sha("another approval");
                r.payload_hash = crate::proof::payload_hash(&r.proof).unwrap();
                r
            }),
        ),
        (
            "a payload hash that is not the proof's",
            Box::new(|mut r| {
                r.payload_hash = sha("other");
                r
            }),
        ),
        (
            "another algorithm",
            Box::new(|mut r| {
                r.algorithm = "rsa".to_owned();
                r
            }),
        ),
    ];
    for (name, tamper) in tampers {
        let (fx, signer) = signed_fx();
        let ctx = ctx();
        let handle = executed(&fx, &ctx).await;
        signer.rewrite_next(tamper);
        let error = gather(&fx, &ctx, &handle).await.unwrap_err();
        assert_eq!(error.full_code(), "policy_denied.proof_refused", "{name}");
        assert!(
            !fx.kinds().contains(&SandboxEventKind::SandboxCollected),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_replayed_collect_asks_for_the_signature_again_and_gets_the_same_record() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    let first = gather(&fx, &ctx, &handle).await.unwrap();
    let again = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(signer.calls(), 2, "the signer is asked each time");
    assert_eq!(signer.inner.len(), 1, "and appends once");
    assert_eq!(
        fx.spy.count(SandboxStep::Collect),
        1,
        "the backend is not asked again"
    );
    // A replay does not hand out the result when the signer cannot answer.
    signer.fail_next(SignerError::Unavailable);
    assert_eq!(
        gather(&fx, &ctx, &handle).await.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );
    signer.fail_next(SignerError::Unavailable);
    let plain = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        plain.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );
}

#[tokio::test]
async fn the_plain_collect_signs_too_and_returns_the_result_only() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    signer.fail_next(SignerError::Unavailable);
    let failed = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await;
    assert_eq!(
        failed.unwrap_err(),
        SandboxError::Unavailable { retriable: true }
    );
    let collected = fx
        .guard
        .collect(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(collected.stdout, b"hello");
    assert_eq!(signer.inner.len(), 1);
}

#[tokio::test]
async fn after_a_restart_the_replayed_node_gets_the_record_signed_before() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    let first = gather(&fx, &ctx, &handle).await.unwrap();

    // The host restarts later; its registry is empty and the node is replayed with the same
    // claim. The program is dated again, the signer answers with the record it holds.
    fx.clock.0.store(NOW + 5_000, Ordering::SeqCst);
    let guard = restarted_signed(&fx, &signer);
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let handle = guard
        .create(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    guard
        .exec(
            &ctx,
            &handle,
            exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    let replayed = guard
        .collect_proved(&ctx, &handle, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(replayed, first);
    assert_eq!(signer.inner.len(), 1);
}

#[tokio::test]
async fn the_proofs_of_one_run_form_one_ordered_chain() {
    let (fx, signer) = signed_fx();
    let mut records = Vec::new();
    for node in ["node-1", "node-2", "node-3"] {
        let ctx = ctx_for("tenant-a", node);
        let handle = executed(&fx, &ctx).await;
        records.push(gather(&fx, &ctx, &handle).await.unwrap().proof);
    }
    assert_eq!(verify_chain(&records, &signer.keys()), Ok(()));
    assert_eq!(
        records[1].prev_hash.as_deref(),
        Some(records[0].payload_hash.as_str())
    );
}

#[tokio::test]
async fn a_program_killed_for_a_limit_is_proved_with_the_partial_outcome() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.spy.script_exec(Err(SandboxError::LimitExceeded {
        kind: crate::error::LimitKind::WallTime,
        partial: Box::new(ExecOutcome {
            declared_exit: DeclaredExit::Killed {
                cause: crate::error::LimitKind::WallTime,
            },
            truncated: true,
            truncation: vec![crate::outcome::Truncation::Stdout],
            declared_duration_ms: 30_000,
        }),
    }));
    let result = run(&fx, &ctx, &handle, &grant).await;
    assert_eq!(code(result), "limit_exceeded.wall_time");
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(
        proved.proof.proof.declared_exit,
        DeclaredExit::Killed {
            cause: crate::error::LimitKind::WallTime
        }
    );
    assert!(proved.proof.proof.truncated);
    assert_eq!(proved.proof.proof.declared_duration_ms, 30_000);
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
}

#[tokio::test]
async fn a_session_returns_the_proof_with_the_result() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let grant = grant_for(&fx, &ctx, &create_req(), &exec_req());
    let inner_grant = grant.clone();
    let never = never();
    let proved = fx
        .guard
        .with_sandbox(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never,
            |session| {
                Box::pin(async move {
                    session
                        .exec(exec_req(), Some(&inner_grant), RUN_STARTED)
                        .await?;
                    session.collect_proved(collect_req(), RUN_STARTED).await
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
}

#[tokio::test]
async fn the_proof_is_truncated_when_only_the_collection_was_cut() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let handle = executed(&fx, &ctx).await;
    // The program's outcome says nothing was cut; the backend cut the collection.
    let mut raw = raw_collected(b"hello", b"");
    raw.declared_truncated = true;
    fx.spy.script_collect(Ok(raw));
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    assert!(proved.collected.truncated);
    assert!(proved.proof.proof.truncated);
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
}

#[tokio::test]
async fn the_limits_that_were_asked_for_are_the_ones_the_proof_states() {
    let (fx, _signer) = signed_fx();
    let ctx = ctx();
    let mut create = create_req();
    create.limits = Limits {
        cpu_millicores: 100,
        memory_bytes: 64 * 1024 * 1024,
        wall_time_ms: 5_000,
        output_bytes: 4_096,
        max_processes: 3,
        ephemeral_disk_bytes: 1024 * 1024,
    };
    assert_ne!(create.limits, Limits::default());
    let grant = grant_for(&fx, &ctx, &create, &exec_req());
    let handle = fx
        .guard
        .create(
            &ctx,
            create.clone(),
            &exec_req(),
            Some(&grant),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(proved.proof.proof.limits_effective, create.limits);
}

/// The proof states the instant the program was sent to the backend, not the instant the call
/// began: the journal append that precedes the backend call may take time.
#[tokio::test]
async fn the_start_of_the_proof_is_the_instant_of_the_backend_call_not_of_the_call() {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *fx.journal.hold.lock().unwrap() =
        Some((SandboxEventKind::SandboxExecStarted, Arc::clone(&gate)));
    fx.clock.0.store(NOW + 5, Ordering::SeqCst);
    let exec = run(&fx, &ctx, &handle, &grant);
    let slow_journal = async {
        fx.journal.held.notified().await;
        fx.clock.0.store(NOW + 30, Ordering::SeqCst);
        gate.notify_one();
    };
    let (result, ()) = tokio::join!(exec, slow_journal);
    result.unwrap();
    *fx.journal.hold.lock().unwrap() = None;
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    let p = &proved.proof.proof;
    assert_eq!(p.started_at, NOW + 30);
    assert!(p.finished_at >= p.started_at);
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
}

/// Every attempt is dated on its own: a retry after an error that left the outcome unknown
/// states the instant of the attempt that answered.
#[tokio::test]
async fn a_retried_exec_is_dated_at_the_attempt_that_answered() {
    let (fx, _signer) = signed_fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    fx.clock.0.store(NOW + 5, Ordering::SeqCst);
    fx.spy.script_exec(Err(SandboxError::Cancelled));
    assert!(run(&fx, &ctx, &handle, &grant).await.is_err());
    let gate = Arc::new(tokio::sync::Notify::new());
    *fx.journal.hold.lock().unwrap() =
        Some((SandboxEventKind::SandboxExecStarted, Arc::clone(&gate)));
    let retry = run(&fx, &ctx, &handle, &grant);
    let slow_journal = async {
        fx.journal.held.notified().await;
        fx.clock.0.store(NOW + 60, Ordering::SeqCst);
        gate.notify_one();
    };
    let (result, ()) = tokio::join!(retry, slow_journal);
    result.unwrap();
    *fx.journal.hold.lock().unwrap() = None;
    let proved = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(proved.proof.proof.started_at, NOW + 60);
}

/// Run one exec that starts at `NOW + 10` while the clock reads `answered` when the backend
/// answers, then collect with the clock at `collected`. Returns the proof's two dates.
async fn dated(answered: u64, collected: u64) -> (u64, u64) {
    let (fx, signer) = signed_fx();
    let ctx = ctx();
    let (handle, grant) = ready(&fx, &ctx).await;
    let clock = Arc::clone(&fx.clock);
    *fx.spy.exec_hook.lock().unwrap() = Some(Box::new(move || {
        clock.0.store(answered, Ordering::SeqCst);
    }));
    fx.clock.0.store(NOW + 10, Ordering::SeqCst);
    run(&fx, &ctx, &handle, &grant).await.unwrap();
    fx.clock.0.store(collected, Ordering::SeqCst);
    // The run is said to have started before every reading, as a host does.
    let started = RUN_STARTED.min(collected);
    let proved = fx
        .guard
        .collect_proved(&ctx, &handle, collect_req(), started, &never())
        .await
        .unwrap();
    assert_eq!(verify_proof(&proved.proof, &signer.keys()), Ok(()));
    let p = proved.proof.proof;
    assert!(p.started_at <= p.finished_at);
    (p.started_at, p.finished_at)
}

fn max_jump() -> u64 {
    let policy = SandboxPolicy::default();
    policy.bounds.max_ttl_ms + policy.max_clock_skew_ms
}

#[tokio::test]
async fn the_end_of_the_run_is_dated_by_the_clock_when_it_moves_normally() {
    let (started, finished) = dated(NOW + 40, NOW + 40).await;
    assert_eq!((started, finished), (NOW + 10, NOW + 40));
}

#[tokio::test]
async fn an_aberrant_jump_forward_is_not_signed_as_the_end_of_the_run() {
    // A day later: more than the longest ttl and the skew allowance in one step.
    let answered = NOW + 10 + 24 * 3_600_000;
    assert!(answered - (NOW + 10) > max_jump());
    let (started, finished) = dated(answered, NOW + 50).await;
    assert_eq!((started, finished), (NOW + 10, NOW + 10));
}

#[tokio::test]
async fn a_clock_set_far_back_never_ends_the_run_before_it_started() {
    // Back by more than the longest ttl and the skew allowance (20 minutes and more).
    let answered = NOW + 10 - max_jump() - 1;
    let (started, finished) = dated(answered, answered).await;
    assert_eq!((started, finished), (NOW + 10, NOW + 10));
}

#[tokio::test]
async fn the_request_digest_covers_the_collect_request() {
    async fn digest_for(collect: CollectRequest) -> String {
        let (fx, _signer) = signed_fx();
        let ctx = ctx();
        let handle = executed(&fx, &ctx).await;
        let proved = fx
            .guard
            .collect_proved(&ctx, &handle, collect, RUN_STARTED, &never())
            .await
            .unwrap();
        proved.proof.proof.request_digest
    }
    let base = digest_for(collect_req()).await;
    assert_eq!(base, digest_for(collect_req()).await, "not deterministic");
    let mut patterns = collect_req();
    patterns.patterns.push("logs/*".to_owned());
    let mut files = collect_req();
    files.max_files += 1;
    let mut bytes = collect_req();
    bytes.max_bytes += 1;
    for other in [patterns, files, bytes] {
        assert_ne!(base, digest_for(other).await);
    }
}
