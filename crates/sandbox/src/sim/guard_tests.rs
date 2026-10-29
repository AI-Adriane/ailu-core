//! The simulated backend driven through the guard: the way a real caller reaches it.

use super::test_util::*;
use super::*;
use crate::approval::VerifiedGrant;
use crate::cancel::CancelToken;
use crate::error::{
    ApprovalSubject, LimitKind, PolicyReason, QuotaKind, SandboxError, SandboxState,
};
use crate::ids::{SandboxHandle, SandboxStep, TenantId};
use crate::journal::SandboxEventKind;
use crate::outcome::{DeclaredExit, ExecOutcome, Truncation};
use crate::policy::{SandboxPolicy, TenantQuota};
use crate::request::{CreateRequest, ExecRequest, HostRule, InputRef, NetworkPolicy};
use crate::{ApprovalClaim, Collected, SandboxContext};
use ailu_fs_backend::ArtifactRef;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

type ExecResult = Result<ExecOutcome, SandboxError>;

fn never() -> CancelToken {
    CancelToken::never()
}

fn code<T: std::fmt::Debug>(result: Result<T, SandboxError>) -> String {
    result.unwrap_err().full_code().to_owned()
}

async fn ready(
    fx: &Fx,
    ctx: &SandboxContext,
    create: CreateRequest,
    exec: &ExecRequest,
) -> (SandboxHandle, ApprovalClaim) {
    let claim = approve(fx, ctx, &create, exec);
    let handle = fx
        .guard
        .create(ctx, create, exec, Some(&claim), RUN_STARTED, &never())
        .await
        .unwrap();
    (handle, claim)
}

async fn run(
    fx: &Fx,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
    exec: ExecRequest,
    claim: &ApprovalClaim,
) -> ExecResult {
    fx.guard
        .exec(ctx, handle, exec, Some(claim), RUN_STARTED, &never())
        .await
}

async fn gather(
    fx: &Fx,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
) -> Result<Collected, SandboxError> {
    fx.guard
        .collect(ctx, handle, collect_req(), RUN_STARTED, &never())
        .await
}

/// Everything the journal and the debug output may show, as one string.
fn visible(fx: &Fx) -> String {
    format!(
        "{} {:?} {:?} {:?}",
        serde_json::to_string(&fx.journal.events()).unwrap(),
        fx.sim,
        fx.secrets,
        fx.net
    )
}

// ---- the cycle ------------------------------------------------------------------------

#[tokio::test]
async fn a_full_cycle_runs_through_the_guard() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    assert_eq!(handle.tenant_id(), &ctx.tenant_id);

    let outcome = run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();
    assert_eq!(outcome.declared_exit, DeclaredExit::Code { code: 0 });
    assert!(!outcome.truncated);
    assert_eq!(outcome.declared_duration_ms, 5);

    let collected = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(collected.stdout, b"hello");
    assert_eq!(collected.stderr, b"warn");
    assert_eq!(collected.files.len(), 1);
    assert_eq!(collected.files[0].path.as_str(), "out/result.txt");
    assert_eq!(collected.files[0].bytes, b"data");
    assert!(!collected.truncated);

    fx.guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert_eq!(fx.sim.program_calls(), 1);
    let kinds: Vec<_> = fx.journal.events().iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SandboxEventKind::SandboxCreated,
            SandboxEventKind::SandboxExecStarted,
            SandboxEventKind::SandboxExecCompleted,
            SandboxEventKind::SandboxCollected,
            SandboxEventKind::SandboxDestroyed,
        ]
    );
    assert!(!visible(&fx).contains("s3cr3t"));
}

#[tokio::test]
async fn with_sandbox_destroys_the_sandbox_when_it_ends() {
    let fx = fx();
    let ctx = ctx();
    let claim = approve(&fx, &ctx, &create_req(), &exec_req());
    let inner = claim.clone();
    let stdout = fx
        .guard
        .with_sandbox(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
            |s| {
                Box::pin(async move {
                    s.exec(exec_req(), Some(&inner), RUN_STARTED).await?;
                    Ok(s.collect(collect_req(), RUN_STARTED).await?.stdout)
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(stdout, b"hello");
    assert_eq!(fx.sim.live_sandboxes(), 0);
}

#[tokio::test]
async fn inputs_are_materialized_from_the_tenants_artifacts() {
    let fx = fx();
    let ctx = ctx();
    let artifact = ArtifactRef {
        id: "run-1:data".into(),
        version: 1,
    };
    let mut create = create_req();
    create.inputs = vec![InputRef {
        artifact: artifact.clone(),
        dest: path("in/data.txt"),
    }];
    let exec = exec_of("cat", &[]);

    // Another tenant's artifact is not found, and the sandbox is not created.
    fx.sim.provide_artifact(
        &TenantId::new("someone-else"),
        artifact.clone(),
        "not yours",
    );
    let claim = approve(&fx, &ctx, &create, &exec);
    let refused = fx
        .guard
        .create(
            &ctx,
            create.clone(),
            &exec,
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await;
    // The backend says `NotFound`; the guard, which reserves that word for itself, relays
    // only a generic error.
    assert_eq!(code(refused), "backend_error");
    assert_eq!(fx.sim.live_sandboxes(), 0);

    fx.sim
        .provide_artifact(&ctx.tenant_id, artifact, "input bytes");
    let handle = fx
        .guard
        .create(&ctx, create, &exec, Some(&claim), RUN_STARTED, &never())
        .await
        .unwrap();
    run(&fx, &ctx, &handle, exec, &claim).await.unwrap();
    assert_eq!(
        gather(&fx, &ctx, &handle).await.unwrap().stdout,
        b"input bytes"
    );
}

// ---- limits and faults ----------------------------------------------------------------

#[tokio::test]
async fn each_limit_kills_the_program_with_the_right_code_and_keeps_the_partial_output() {
    let cases = [
        ("sleep", "limit_exceeded.wall_time", LimitKind::WallTime),
        ("spin", "limit_exceeded.wall_time", LimitKind::WallTime),
        ("alloc", "limit_exceeded.memory", LimitKind::Memory),
        ("fork", "limit_exceeded.processes", LimitKind::Processes),
        ("fill", "limit_exceeded.disk", LimitKind::Disk),
    ];
    for (program, expected, kind) in cases {
        let fx = fx();
        let ctx = ctx();
        let mut create = create_req();
        create.limits.ephemeral_disk_bytes = 4096;
        let exec = exec_of(program, &[]);
        let (handle, claim) = ready(&fx, &ctx, create, &exec).await;
        let error = run(&fx, &ctx, &handle, exec, &claim).await.unwrap_err();
        assert_eq!(error.full_code(), expected, "{program}");
        let SandboxError::LimitExceeded { partial, .. } = error else {
            panic!("{program}: not a limit error");
        };
        assert_eq!(partial.declared_exit, DeclaredExit::Killed { cause: kind });
        // What was produced before the kill can still be collected.
        let collected = gather(&fx, &ctx, &handle).await.unwrap();
        if program == "alloc" {
            assert_eq!(collected.stdout, b"partial");
        }
        if program == "fill" {
            assert!(!collected.files.is_empty() && collected.files.len() <= 4);
        }
        fx.guard.destroy(&ctx, &handle).await.unwrap();
    }
}

#[tokio::test]
async fn a_scripted_limit_kills_a_program_that_would_have_succeeded() {
    for (kind, expected) in [
        (LimitKind::WallTime, "limit_exceeded.wall_time"),
        (LimitKind::Memory, "limit_exceeded.memory"),
        (LimitKind::Processes, "limit_exceeded.processes"),
        (LimitKind::Disk, "limit_exceeded.disk"),
    ] {
        let fx = fx();
        let ctx = ctx();
        let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
        fx.sim
            .inject(SandboxStep::Exec, SimFault::Limit(kind))
            .unwrap();
        let result = run(&fx, &ctx, &handle, exec_req(), &claim).await;
        assert_eq!(code(result), expected);
        assert_eq!(fx.sim.program_calls(), 1);
        assert_eq!(gather(&fx, &ctx, &handle).await.unwrap().stdout, b"hello");
    }
}

#[tokio::test]
async fn a_huge_output_is_truncated_and_never_buffered() {
    for scripted in [false, true] {
        let fx = fx();
        let ctx = ctx();
        let mut create = create_req();
        create.limits.output_bytes = 1000;
        let exec = if scripted {
            exec_req()
        } else {
            exec_of("flood", &[])
        };
        let (handle, claim) = ready(&fx, &ctx, create, &exec).await;
        if scripted {
            fx.sim
                .inject(SandboxStep::Exec, SimFault::HugeOutput)
                .unwrap();
        }
        let outcome = run(&fx, &ctx, &handle, exec, &claim).await.unwrap();
        assert!(outcome.truncated);
        assert_eq!(outcome.truncation, vec![Truncation::Stdout]);
        let collected = gather(&fx, &ctx, &handle).await.unwrap();
        assert_eq!(collected.stdout.len(), 1000, "bounded by output_bytes");
        assert!(collected.truncated);
    }
}

#[tokio::test]
async fn unavailable_busy_and_other_scripted_errors_surface_with_their_codes() {
    // Unavailable { retriable: true }: nothing ran, asking again runs the program once.
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::unavailable(true))
        .unwrap();
    let error = run(&fx, &ctx, &handle, exec_req(), &claim)
        .await
        .unwrap_err();
    assert_eq!(error, SandboxError::Unavailable { retriable: true });
    assert!(error.is_retriable());
    assert_eq!(fx.sim.program_calls(), 0);
    run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();
    assert_eq!(fx.sim.program_calls(), 1);

    // Unavailable { retriable: false }: never retried, the program never runs.
    let fx = self::fx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::unavailable(false))
        .unwrap();
    for _ in 0..3 {
        let error = run(&fx, &ctx, &handle, exec_req(), &claim)
            .await
            .unwrap_err();
        assert_eq!(error, SandboxError::Unavailable { retriable: false });
        assert!(!error.is_retriable());
    }
    assert_eq!(fx.sim.program_calls(), 0);

    // Busy keeps its hint, and is a wait: the next call works.
    let fx = self::fx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::busy(Some(5)))
        .unwrap();
    let error = run(&fx, &ctx, &handle, exec_req(), &claim)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SandboxError::Busy {
            retry_after_ms: Some(5)
        }
    );
    run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();

    // The same at create and at collect.
    let fx = self::fx();
    fx.sim
        .inject(SandboxStep::Create, SimFault::unavailable(true))
        .unwrap();
    let claim = approve(&fx, &ctx, &create_req(), &exec_req());
    let created = fx
        .guard
        .create(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(created), "unavailable");
    let handle = fx
        .guard
        .create(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();
    fx.sim
        .inject(SandboxStep::Collect, SimFault::busy(None))
        .unwrap();
    assert_eq!(code(gather(&fx, &ctx, &handle).await), "busy");
    gather(&fx, &ctx, &handle).await.unwrap();
}

#[tokio::test]
async fn an_isolation_violation_poisons_the_sandbox() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::violation("escape attempt"))
        .unwrap();
    let error = run(&fx, &ctx, &handle, exec_req(), &claim)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "isolation_violation");
    // The signal is the guard's own fixed word, not the backend's text.
    assert!(!format!("{error:?}").contains("escape"));
    assert_eq!(
        code(gather(&fx, &ctx, &handle).await),
        "invalid_state",
        "a poisoned sandbox accepts only destroy"
    );
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert!(fx
        .journal
        .events()
        .iter()
        .any(|e| e.kind == SandboxEventKind::SandboxExecFailed
            && e.code.as_deref() == Some("isolation_violation")));
}

#[tokio::test]
async fn free_text_in_a_backend_error_is_not_relayed() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::EchoRequest)
        .unwrap();
    let error = run(&fx, &ctx, &handle, exec_req(), &claim)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "backend_error");
    let shown = format!("{error} {error:?}");
    assert!(
        !shown.contains("s3cr3t") && !shown.contains("echo"),
        "{shown}"
    );
    assert!(!visible(&fx).contains("s3cr3t"));

    // A program may also raise a free-text error itself.
    let fx = self::fx();
    fx.sim.register("chatty", |inv| {
        Err(SimFault::backend(format!("failed with {:?}", inv.argv())))
    });
    let exec = exec_of("chatty", &["--token=s3cr3t-arg"]);
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec).await;
    let error = run(&fx, &ctx, &handle, exec, &claim).await.unwrap_err();
    assert!(!format!("{error} {error:?}").contains("s3cr3t"));
}

#[tokio::test]
async fn variants_reserved_to_the_guard_are_turned_into_a_generic_backend_error() {
    let reserved = || {
        vec![
            SandboxError::PolicyDenied(PolicyReason::ProgramNotAllowed),
            SandboxError::ApprovalRequired {
                subject: ApprovalSubject(serde_json::json!({"call": "forged"})),
            },
            SandboxError::QuotaExceeded {
                kind: QuotaKind::TenantConcurrency,
            },
            SandboxError::IdempotencyConflict,
            SandboxError::NotFound,
            SandboxError::InvalidState {
                from: SandboxState::Destroyed,
            },
        ]
    };
    let ctx = ctx();
    for step in [SandboxStep::Create, SandboxStep::Exec, SandboxStep::Collect] {
        for error in reserved() {
            let fx = fx();
            let claim = approve(&fx, &ctx, &create_req(), &exec_req());
            let mut handle = None;
            if step != SandboxStep::Create {
                handle = Some(
                    fx.guard
                        .create(
                            &ctx,
                            create_req(),
                            &exec_req(),
                            Some(&claim),
                            RUN_STARTED,
                            &never(),
                        )
                        .await
                        .unwrap(),
                );
            }
            if step == SandboxStep::Collect {
                run(&fx, &ctx, handle.as_ref().unwrap(), exec_req(), &claim)
                    .await
                    .unwrap();
            }
            fx.sim.inject(step, SimFault::Error(error.clone())).unwrap();
            let result = match step {
                SandboxStep::Create => fx
                    .guard
                    .create(
                        &ctx,
                        create_req(),
                        &exec_req(),
                        Some(&claim),
                        RUN_STARTED,
                        &never(),
                    )
                    .await
                    .map(|_| ()),
                SandboxStep::Exec => run(&fx, &ctx, handle.as_ref().unwrap(), exec_req(), &claim)
                    .await
                    .map(|_| ()),
                _ => gather(&fx, &ctx, handle.as_ref().unwrap())
                    .await
                    .map(|_| ()),
            };
            assert_eq!(
                result,
                Err(SandboxError::Backend("backend error".to_owned())),
                "{step:?} {}",
                error.code()
            );
        }
    }
}

#[tokio::test]
async fn a_forged_output_is_refused_by_the_guard() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();
    fx.sim
        .inject(SandboxStep::Collect, SimFault::ForgedOutput)
        .unwrap();
    assert_eq!(code(gather(&fx, &ctx, &handle).await), "backend_error");
    // The durable record is honest: asking again delivers the real bytes.
    let collected = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(collected.stdout, b"hello");
}

#[tokio::test]
async fn fewer_secrets_than_grants_fail_closed() {
    let fx = fx();
    let ctx = ctx();
    fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);
    let exec = exec_of("leak", &[]);
    let (handle, claim) = ready(&fx, &ctx, create_with_secret(), &exec).await;
    run(&fx, &ctx, &handle, exec, &claim).await.unwrap();
    fx.sim
        .inject(SandboxStep::Collect, SimFault::FewerSecrets)
        .unwrap();
    assert_eq!(code(gather(&fx, &ctx, &handle).await), "backend_error");
    let collected = gather(&fx, &ctx, &handle).await.unwrap();
    assert_eq!(collected.stdout, b"key=[REDACTED]");
}

#[tokio::test]
async fn a_secret_reaches_the_program_and_nothing_else() {
    let fx = fx();
    let ctx = ctx();
    fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);

    // The environment holds the fixed base set, plus the granted name.
    let names = exec_of("names", &[]);
    let (handle, claim) = ready(&fx, &ctx, create_with_secret(), &names).await;
    run(&fx, &ctx, &handle, names, &claim).await.unwrap();
    assert_eq!(
        gather(&fx, &ctx, &handle).await.unwrap().stdout,
        b"API_KEY,LANG,PATH"
    );

    let ctx2 = ctx_for("tenant-a", "node-2");
    let bare = exec_of("names", &[]);
    let (handle, claim) = ready(&fx, &ctx2, create_req(), &bare).await;
    run(&fx, &ctx2, &handle, bare, &claim).await.unwrap();
    assert_eq!(
        gather(&fx, &ctx2, &handle).await.unwrap().stdout,
        b"LANG,PATH"
    );

    // A program that prints the value everywhere is redacted by the guard.
    let ctx3 = ctx_for("tenant-a", "node-3");
    let leak = exec_of("leak", &[]);
    let (handle, claim) = ready(&fx, &ctx3, create_with_secret(), &leak).await;
    run(&fx, &ctx3, &handle, leak, &claim).await.unwrap();
    let collected = gather(&fx, &ctx3, &handle).await.unwrap();
    assert_eq!(collected.stdout, b"key=[REDACTED]");
    assert_eq!(collected.stderr, b"[REDACTED]");
    assert_eq!(collected.files[0].bytes, b"[REDACTED]");
    assert_eq!(
        fx.secrets.resolutions(),
        4,
        "one per exec and per collect that holds a grant"
    );

    let shown = visible(&fx);
    assert!(!shown.contains(SECRET), "{shown}");
    assert!(!format!("{collected:?}").contains(SECRET));
}

#[tokio::test]
async fn another_tenants_secret_reference_is_refused_before_anything_runs() {
    let fx = fx();
    fx.secrets
        .set(&TenantId::new("tenant-a"), "api-key", SECRET);
    let intruder = ctx_for("tenant-b", "node-1");
    let exec = exec_of("leak", &[]);
    let (handle, claim) = ready(&fx, &intruder, create_with_secret(), &exec).await;
    let result = run(&fx, &intruder, &handle, exec, &claim).await;
    assert_eq!(code(result), "backend_error", "the refusal is generic");
    assert_eq!(fx.sim.program_calls(), 0);
    assert!(!visible(&fx).contains(SECRET));
}

/// The guard refuses standard input: it is not part of what an approver can read in full, so the
/// simulator is never reached with it.
#[tokio::test]
async fn standard_input_is_refused_before_the_simulator_is_reached() {
    let fx = fx();
    let ctx = ctx();
    let mut exec = exec_of("echo", &[]);
    exec.stdin = Some(format!("stdin-{SECRET}").into_bytes());
    let result = fx
        .guard
        .create(
            &ctx,
            create_with_secret(),
            &exec,
            None,
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(result), "policy_denied.stdin_not_allowed");
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert_eq!(fx.sim.program_calls(), 0);
    assert!(!visible(&fx).contains("stdin-"));
}

#[tokio::test]
async fn the_environment_debug_output_hides_arguments_and_secrets() {
    let fx = fx();
    let ctx = ctx();
    fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);
    let exec = exec_of("debug-self", &["--token=s3cr3t-arg"]);
    let (handle, claim) = ready(&fx, &ctx, create_with_secret(), &exec).await;
    run(&fx, &ctx, &handle, exec, &claim).await.unwrap();
    let collected = gather(&fx, &ctx, &handle).await.unwrap();
    let shown = String::from_utf8(collected.stdout).unwrap();
    assert!(
        !shown.contains("s3cr3t") && !shown.contains(SECRET),
        "{shown}"
    );
    assert!(
        shown.contains("env_vars: 3") && shown.contains("argc: 2"),
        "{shown}"
    );
    for text in [
        format!("{:?}", fx.sim),
        format!("{:?}", fx.secrets),
        format!("{:?}", fx.net),
        format!("{:#?}", SimFault::backend("s3cr3t")),
    ] {
        assert!(!text.contains("s3cr3t"), "{text}");
    }
}

// ---- idempotence ----------------------------------------------------------------------

#[tokio::test]
async fn twenty_concurrent_execs_with_one_key_call_the_program_once() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let guard = Arc::clone(&fx.guard);
        let (ctx, handle, claim) = (ctx.clone(), handle.clone(), claim.clone());
        tasks.push(tokio::spawn(async move {
            guard
                .exec(
                    &ctx,
                    &handle,
                    exec_req(),
                    Some(&claim),
                    RUN_STARTED,
                    &CancelToken::never(),
                )
                .await
        }));
    }
    let mut done = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(outcome) => {
                assert_eq!(outcome.declared_exit, DeclaredExit::Code { code: 0 });
                done += 1;
            }
            Err(error) => assert_eq!(error.code(), "busy"),
        }
    }
    assert!(done >= 1);
    assert_eq!(
        fx.sim.program_calls(),
        1,
        "one execution, whatever the interleaving"
    );
}

#[tokio::test]
async fn an_execution_resumes_after_a_cancellation() {
    // The token says "cancelled" on its n-th question: at the guard, then at the backend's
    // first check, then at its second (after the secrets were resolved).
    for n in 1..=3u32 {
        let fx = fx();
        let ctx = ctx();
        fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);
        let exec = exec_of("leak", &[]);
        let (handle, claim) = ready(&fx, &ctx, create_with_secret(), &exec).await;
        let asked = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&asked);
        let token = CancelToken::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst) + 1 == n
        }));
        let cancelled = fx
            .guard
            .exec(
                &ctx,
                &handle,
                exec.clone(),
                Some(&claim),
                RUN_STARTED,
                &token,
            )
            .await;
        assert_eq!(code(cancelled), "cancelled", "n = {n}");
        assert_eq!(
            fx.sim.program_calls(),
            0,
            "cancelled before any effect (n = {n})"
        );
        // The same exec, with the same key, resumes and runs exactly once.
        run(&fx, &ctx, &handle, exec.clone(), &claim).await.unwrap();
        run(&fx, &ctx, &handle, exec, &claim).await.unwrap();
        assert_eq!(fx.sim.program_calls(), 1, "n = {n}");
    }
}

#[tokio::test]
async fn a_marker_without_a_result_is_unavailable_and_never_reruns_the_program() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Exec, SimFault::CrashAfterMarker)
        .unwrap();
    // The host died after the program ran and before its result was kept.
    let first = run(&fx, &ctx, &handle, exec_req(), &claim)
        .await
        .unwrap_err();
    assert_eq!(first, SandboxError::Unavailable { retriable: true });
    assert_eq!(fx.sim.program_calls(), 1);
    // Asking again finds the marker and no result: final, and no second run.
    for _ in 0..3 {
        let again = run(&fx, &ctx, &handle, exec_req(), &claim)
            .await
            .unwrap_err();
        assert_eq!(again, SandboxError::Unavailable { retriable: false });
    }
    assert_eq!(fx.sim.program_calls(), 1);
    assert_eq!(code(gather(&fx, &ctx, &handle).await), "invalid_state");

    // A restarted host (a fresh guard) gets the same answer from the durable record.
    let guard = restarted(&fx);
    let handle = guard
        .create(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    let after_restart = guard
        .exec(
            &ctx,
            &handle,
            exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        after_restart,
        SandboxError::Unavailable { retriable: false }
    );
    assert_eq!(fx.sim.program_calls(), 1);
}

#[tokio::test]
async fn a_program_that_panics_leaves_a_marker_and_is_not_rerun() {
    let fx = fx();
    let ctx = ctx();
    let exec = exec_of("panic", &[]);
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec).await;
    let error = run(&fx, &ctx, &handle, exec.clone(), &claim)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "backend_error");
    assert!(!format!("{error:?}").contains("the program failed"));
    let guard = restarted(&fx);
    let handle = guard
        .create(
            &ctx,
            create_req(),
            &exec,
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    let again = guard
        .exec(&ctx, &handle, exec, Some(&claim), RUN_STARTED, &never())
        .await
        .unwrap_err();
    assert_eq!(again, SandboxError::Unavailable { retriable: false });
    assert_eq!(fx.sim.program_calls(), 1);
}

#[tokio::test]
async fn a_replay_after_destroy_inside_the_retention_returns_the_stored_answers() {
    let fx = fx();
    let ctx = ctx();
    fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);
    let exec = exec_of("leak", &[]);
    let (handle, claim) = ready(&fx, &ctx, create_with_secret(), &exec).await;
    let first = run(&fx, &ctx, &handle, exec.clone(), &claim).await.unwrap();
    let collected = gather(&fx, &ctx, &handle).await.unwrap();
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(fx.sim.live_sandboxes(), 0);

    // The node is replayed by a restarted host, one hour later: no sandbox exists any more.
    fx.clock.advance(3_600_000);
    let guard = restarted(&fx);
    let replayed = guard
        .create(
            &ctx,
            create_with_secret(),
            &exec,
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    assert_eq!(
        replayed.sandbox_id(),
        handle.sandbox_id(),
        "same key, same sandbox"
    );
    let again = guard
        .exec(&ctx, &replayed, exec, Some(&claim), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(again, first);
    let again = guard
        .collect(&ctx, &replayed, collect_req(), RUN_STARTED, &never())
        .await
        .unwrap();
    assert_eq!(
        again, collected,
        "the secret is resolved again to redact it"
    );
    assert_eq!(again.stdout, b"key=[REDACTED]");
    assert_eq!(fx.sim.program_calls(), 1, "nothing ran twice");
}

#[tokio::test]
async fn an_attempt_older_than_the_retention_is_refused_before_any_effect() {
    let fx = fx();
    let ctx = ctx();
    let (handle, claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    run(&fx, &ctx, &handle, exec_req(), &claim).await.unwrap();
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    fx.clock.advance(8 * 24 * 3_600_000);
    // The durable record is gone after the sweep, and the guard refuses what it could not
    // tell from a call that never ran.
    let swept = fx.sim.sweep();
    assert_eq!(swept.records, 2, "the create and the exec records");
    assert_eq!(fx.sim.stored_records(&ctx.tenant_id), 0);
    let calls = fx.sim.program_calls();
    let replay = fx
        .guard
        .exec(
            &ctx,
            &handle,
            exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(replay), "policy_denied.attempt_too_old");
    assert_eq!(fx.sim.program_calls(), calls);
}

// ---- ttl ------------------------------------------------------------------------------

#[tokio::test]
async fn a_sandbox_left_behind_is_swept_after_its_ttl() {
    let fx = fx();
    let ctx = ctx();
    let (handle, _claim) = ready(&fx, &ctx, create_req(), &exec_req()).await;
    assert_eq!((fx.sim.live_sandboxes(), fx.sim.residue()), (1, 0));
    fx.clock.advance(59_999);
    assert_eq!(fx.sim.residue(), 0, "not yet");
    fx.clock.advance(1);
    assert_eq!(fx.sim.residue(), 1, "a resource past its ttl is a residue");
    let report = fx.sim.sweep();
    assert_eq!(report.sandboxes, 1);
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert_eq!(fx.sim.residue(), 0, "nothing is left past its ttl");
    assert_eq!(
        fx.sim.stored_records(&ctx.tenant_id),
        1,
        "the create record remains"
    );
    // Destroying what is already gone succeeds.
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    // The records follow at the end of the retention.
    fx.clock.advance(fx.sim.retention_ms());
    assert!(fx.sim.sweep().records >= 1);
    assert_eq!(fx.sim.residue(), 0);
    assert_eq!(fx.sim.stored_records(&ctx.tenant_id), 0);
}

// ---- tenants --------------------------------------------------------------------------

#[tokio::test]
async fn two_tenants_in_parallel_never_share_an_isolation_domain() {
    let fx = fx();
    let mut tasks = Vec::new();
    for tenant in ["tenant-a", "tenant-b"] {
        for n in 0..3 {
            let guard = Arc::clone(&fx.guard);
            let verifier = Arc::clone(&fx.verifier);
            let ctx = ctx_for(tenant, &format!("node-{n}"));
            tasks.push(tokio::spawn(async move {
                let request = guard
                    .request_approval(&ctx, &create_req(), &exec_req())
                    .unwrap();
                verifier.approve(VerifiedGrant {
                    approval_id: format!("apr-{}", &request.call_key[13..29]),
                    call_key: request.call_key.clone(),
                    tenant_id: ctx.tenant_id.clone(),
                    run_id: ctx.run_id.clone(),
                    requested_by: "agent".to_owned(),
                    approved_by: "alice".to_owned(),
                    approval_payload_hash: "ab".repeat(32),
                });
                let claim = ApprovalClaim {
                    approval_id: format!("apr-{}", &request.call_key[13..29]),
                    call_key: request.call_key,
                };
                guard
                    .create(
                        &ctx,
                        create_req(),
                        &exec_req(),
                        Some(&claim),
                        RUN_STARTED,
                        &CancelToken::never(),
                    )
                    .await
                    .unwrap()
            }));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for task in tasks {
        task.await.unwrap();
    }
    for event in fx.journal.events() {
        let domain = event
            .isolation_domain
            .expect("a created event names its domain");
        assert!(seen.insert(domain.clone()), "a domain is never given twice");
        assert_eq!(
            fx.sim.domain_owner(domain.as_str()),
            Some(event.tenant_id.clone()),
            "and it belongs to the tenant that got it"
        );
    }
    assert_eq!(seen.len(), 6);
}

#[tokio::test]
async fn a_backend_that_shares_a_domain_is_caught_by_the_guard() {
    let fx = fx();
    let a = ctx_for("tenant-a", "node-1");
    let b = ctx_for("tenant-b", "node-1");
    let _ = ready(&fx, &a, create_req(), &exec_req()).await;
    fx.sim
        .inject(SandboxStep::Create, SimFault::SharedDomain)
        .unwrap();
    let claim = approve(&fx, &b, &create_req(), &exec_req());
    let result = fx
        .guard
        .create(
            &b,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(result), "isolation_violation");
    // The guard destroyed the sandbox it could not trust, and left tenant A's alone.
    assert_eq!(fx.sim.live_sandboxes(), 1);
}

#[tokio::test]
async fn a_tenant_over_its_quota_is_refused_at_once_and_others_are_served() {
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota = TenantQuota {
        max_concurrent: 2,
        max_creations: 3,
        window_ms: 60_000,
    };
    let fx = fx_with(policy);
    let a = |n: u32| ctx_for("tenant-a", &format!("node-{n}"));
    let mut handles = Vec::new();
    for n in 1..=2 {
        handles.push(ready(&fx, &a(n), create_req(), &exec_req()).await.0);
    }
    let claim = approve(&fx, &a(3), &create_req(), &exec_req());
    let before = fx.sim.live_sandboxes();
    let over = fx
        .guard
        .create(
            &a(3),
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(over), "quota_exceeded.tenant_concurrency");
    assert_eq!(
        fx.sim.live_sandboxes(),
        before,
        "the backend was not reached"
    );

    // Another tenant is not slowed down by it.
    let _ = ready(
        &fx,
        &ctx_for("tenant-b", "node-1"),
        create_req(),
        &exec_req(),
    )
    .await;

    // Giving a slot back lets the next create through, until the rate ceiling.
    fx.guard.destroy(&a(1), &handles[0]).await.unwrap();
    fx.guard
        .create(
            &a(3),
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    fx.guard.destroy(&a(2), &handles[1]).await.unwrap();
    let claim4 = approve(&fx, &a(4), &create_req(), &exec_req());
    let rate = fx
        .guard
        .create(
            &a(4),
            create_req(),
            &exec_req(),
            Some(&claim4),
            RUN_STARTED,
            &never(),
        )
        .await;
    assert_eq!(code(rate), "quota_exceeded.creation_rate");
    fx.clock.advance(61_000);
    fx.guard
        .create(
            &a(4),
            create_req(),
            &exec_req(),
            Some(&claim4),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_handle_of_another_tenant_is_not_found() {
    let fx = fx();
    let a = ctx_for("tenant-a", "node-1");
    let (handle, claim) = ready(&fx, &a, create_req(), &exec_req()).await;
    let b = ctx_for("tenant-b", "node-1");
    let exec = fx
        .guard
        .exec(&b, &handle, exec_req(), Some(&claim), RUN_STARTED, &never())
        .await;
    assert_eq!(code(exec), "not_found");
    assert_eq!(code(gather(&fx, &b, &handle).await), "not_found");
    assert_eq!(code(fx.guard.destroy(&b, &handle).await), "not_found");
    assert_eq!(fx.sim.live_sandboxes(), 1);
}

// ---- network --------------------------------------------------------------------------

fn network_fixture() -> (Fx, Vec<HostRule>) {
    let rules = reference::rules();
    let mut policy = SandboxPolicy::default();
    policy.network_allow = rules.clone();
    policy.tenant_quota.max_creations = 1000;
    let fx = fx_with(policy);
    reference::install(&fx.net);
    (fx, rules)
}

/// Run `program host port` in a fresh sandbox with the given network policy; return what the
/// program printed.
async fn probe(
    fx: &Fx,
    network: NetworkPolicy,
    n: u32,
    program: &str,
    host: &str,
    port: &str,
) -> String {
    let ctx = ctx_for("tenant-a", &format!("probe-{n}"));
    let mut create = create_req();
    create.network = network;
    let exec = exec_of(program, &[host, port]);
    let (handle, claim) = ready(fx, &ctx, create, &exec).await;
    run(fx, &ctx, &handle, exec, &claim).await.unwrap();
    let said = gather(fx, &ctx, &handle).await.unwrap().stdout;
    fx.guard.destroy(&ctx, &handle).await.unwrap();
    String::from_utf8(said).unwrap()
}

#[tokio::test]
async fn network_is_denied_by_default_dns_included() {
    let (fx, _) = network_fixture();
    let said = probe(
        &fx,
        NetworkPolicy::Deny,
        1,
        "fetch",
        "api.example.com",
        "443",
    )
    .await;
    assert_eq!(said, "network denied");
    let said = probe(
        &fx,
        NetworkPolicy::Deny,
        2,
        "resolve",
        "api.example.com",
        "0",
    )
    .await;
    assert_eq!(said, "network denied");
    let stats = fx.net.stats();
    assert_eq!((stats.network_denied, stats.allowed), (2, 0));
}

#[tokio::test]
async fn the_network_policy_is_applied_to_hosts_redirects_and_addresses() {
    let (fx, rules) = network_fixture();
    let allow = || NetworkPolicy::Allow(rules.clone());
    let cases = [
        ("api.example.com", "443", "ok:93.184.216.34"),
        ("other.example.com", "443", "host not allowed"),
        ("api.example.com", "80", "host not allowed"),
        ("hop.example.com", "80", "ok:93.184.216.34"),
        ("badhop.example.com", "80", "redirect target not allowed"),
        ("private.example.com", "443", "address blocked"),
        ("loopback.example.com", "443", "address blocked"),
        ("linklocal.example.com", "443", "address blocked"),
        ("ula.example.com", "443", "address blocked"),
        ("mapped.example.com", "443", "address blocked"),
        ("meta4.example.com", "443", "address blocked"),
        ("meta6.example.com", "443", "address blocked"),
        ("rebind.example.com", "443", "dns rebinding"),
    ];
    for (n, (host, port, expected)) in cases.into_iter().enumerate() {
        let n = u32::try_from(n).unwrap();
        let said = probe(&fx, allow(), n, "fetch", host, port).await;
        assert_eq!(said, expected, "{host}:{port}");
    }
    let stats = fx.net.stats();
    assert_eq!(stats.allowed, 2);
    assert_eq!(stats.host_not_allowed, 2);
    assert_eq!(stats.redirect_not_allowed, 1);
    assert_eq!(stats.address_blocked, 7);
    assert_eq!(stats.metadata_blocked, 2);
    assert_eq!(stats.rebinding, 1);
    assert_eq!(stats.denied(), 11);
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_is_refused_in_dns_too() {
    let (fx, rules) = network_fixture();
    let allow = || NetworkPolicy::Allow(rules.clone());
    let said = probe(&fx, allow(), 1, "resolve", "private.example.com", "0").await;
    assert_eq!(said, "address blocked");
    let said = probe(&fx, allow(), 2, "resolve", "api.example.com", "0").await;
    assert_eq!(said, "ok:1");
    let said = probe(&fx, allow(), 3, "resolve", "other.example.com", "0").await;
    assert_eq!(said, "host not allowed");
}

#[tokio::test]
async fn an_allowed_host_that_relays_passes_and_is_counted() {
    let (fx, rules) = network_fixture();
    let said = probe(
        &fx,
        NetworkPolicy::Allow(rules),
        1,
        "fetch",
        "relay.example.com",
        "443",
    )
    .await;
    assert_eq!(
        said, "ok:93.184.216.39",
        "the accepted limit of an allow list"
    );
    let stats = fx.net.stats();
    assert_eq!((stats.relayed, stats.denied()), (1, 0));
}

// ---- the feature ----------------------------------------------------------------------

/// The `[section]` of a manifest, without its header.
fn manifest_section<'a>(manifest: &'a str, name: &str) -> &'a str {
    let header = format!("[{name}]");
    let start = manifest
        .find(&header)
        .map_or("", |i| &manifest[i + header.len()..]);
    start.split("\n[").next().unwrap_or("")
}

#[test]
fn the_sim_feature_is_off_by_default_and_pulls_nothing_in() {
    let manifest = include_str!("../../Cargo.toml");
    let features = manifest_section(manifest, "features");
    assert!(
        features.lines().any(|l| l.replace(' ', "") == "sim=[]"),
        "sim is declared and enables nothing: {features}"
    );
    assert!(
        !features
            .lines()
            .any(|l| l.trim_start().starts_with("default")),
        "no default feature set exists, so sim is off: {features}"
    );
    // No runtime dependency on an async runtime or an HTTP client.
    let deps = manifest_section(manifest, "dependencies");
    for forbidden in ["tokio", "reqwest", "hyper", "ureq", "isahc"] {
        assert!(!deps.contains(forbidden), "{forbidden} is not a dependency");
    }
}

#[test]
fn the_simulator_launches_nothing_and_opens_no_socket() {
    // A second net under the lints of `clippy.toml`, which see through any import form.
    // Patterns are assembled so that this file does not match itself.
    let forbidden = [
        ["std::", "process"].concat(),
        ["tokio::", "process"].concat(),
        ["Command", "::new"].concat(),
        ["un", "safe"].concat(),
        ["ev", "al("].concat(),
        ["libc", "::"].concat(),
        ["Tcp", "Stream"].concat(),
        ["Udp", "Socket"].concat(),
        ["ToSocket", "Addrs"].concat(),
        ["dlo", "pen"].concat(),
        // Grouped imports hide the path from the patterns above: `use std::{process}`.
        ["process", "::"].concat(),
        ["{", "process"].concat(),
        ["process", "}"].concat(),
        [", ", "process"].concat(),
        ["std::{", "fs"].concat(),
        ["std::fs", "::"].concat(),
        ["std::en", "v"].concat(),
        // The other doors the lints of `clippy.toml` close: a local socket, a thread other than
        // `spawn`, a name resolution, a read of the host's input or path, in any spelling.
        ["std::", "os"].concat(),
        ["std::{", "os"].concat(),
        ["os::", "unix"].concat(),
        ["Unix", "Stream"].concat(),
        ["Unix", "Listener"].concat(),
        ["Unix", "Datagram"].concat(),
        ["thread::", "scope"].concat(),
        ["thread::", "Builder"].concat(),
        ["to_socket", "_addrs"].concat(),
        ["std::", "io"].concat(),
        ["std::{", "io"].concat(),
        ["std::{", "thread"].concat(),
        ["std::{", "net"].concat(),
        ["read", "_link"].concat(),
        ["canon", "icalize"].concat(),
        ["hard", "_link"].concat(),
        ["current", "_dir"].concat(),
        ["set", "_var"].concat(),
    ];
    let sources = [
        include_str!("backend.rs"),
        include_str!("clock.rs"),
        include_str!("fault.rs"),
        include_str!("fs.rs"),
        include_str!("mod.rs"),
        include_str!("net.rs"),
        include_str!("output.rs"),
        include_str!("program.rs"),
        include_str!("secrets.rs"),
    ];
    for source in sources {
        for pattern in &forbidden {
            assert!(!source.contains(pattern.as_str()), "found {pattern:?}");
        }
    }
}

#[test]
fn a_fault_is_refused_at_a_step_it_cannot_happen_at() {
    let sim = SimBackend::new(Arc::new(SimClock::new(NOW)));
    assert_eq!(
        sim.inject(SandboxStep::Exec, SimFault::ForgedOutput),
        Err(FaultNotApplicable)
    );
    assert_eq!(
        sim.inject(SandboxStep::Collect, SimFault::Limit(LimitKind::Disk)),
        Err(FaultNotApplicable)
    );
    assert!(sim
        .inject(SandboxStep::Destroy, SimFault::unavailable(true))
        .is_ok());
}

#[tokio::test]
async fn a_create_for_a_run_that_is_too_old_never_reaches_the_simulator() {
    let fx = fx();
    let ctx = ctx();
    let claim = approve(&fx, &ctx, &create_req(), &exec_req());
    // A run that started more than the retention ago, and one that starts in the future.
    for started in [NOW - 25 * 3_600_000, NOW + 3_600_000] {
        let result = fx
            .guard
            .create(
                &ctx,
                create_req(),
                &exec_req(),
                Some(&claim),
                started,
                &never(),
            )
            .await;
        let expected = if started < NOW {
            "policy_denied.attempt_too_old"
        } else {
            "policy_denied.attempt_in_future"
        };
        assert_eq!(code(result), expected);
    }
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert_eq!(fx.sim.stored_records(&ctx.tenant_id), 0);
    assert_eq!(fx.sim.program_calls(), 0);
    // The same call for a recent run goes through.
    fx.guard
        .create(
            &ctx,
            create_req(),
            &exec_req(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_approval_subject_shows_the_whole_call_and_a_call_too_big_to_read_is_refused() {
    let fx = fx();
    let ctx = ctx();
    fx.secrets.set(&ctx.tenant_id, "api-key", SECRET);

    // A call that cannot be read whole is refused before any approval is asked for, and the
    // simulator sees nothing.
    let mut too_many = exec_of("echo", &[]);
    too_many.argv.extend((0..32).map(|n| format!("arg-{n}")));
    assert_eq!(too_many.argv.len(), 33);
    let mut too_long = exec_of("echo", &[]);
    too_long.argv.push("x".repeat(257));
    for exec in [too_many, too_long] {
        let result = fx
            .guard
            .create(
                &ctx,
                create_with_secret(),
                &exec,
                None,
                RUN_STARTED,
                &never(),
            )
            .await;
        assert_eq!(code(result), "policy_denied.unreviewable_call");
    }
    assert_eq!(fx.sim.live_sandboxes(), 0);
    assert_eq!(fx.sim.stored_records(&ctx.tenant_id), 0);
    assert_eq!(fx.sim.program_calls(), 0);

    // 32 arguments of 256 characters are accepted, and shown in full.
    let mut exec = exec_of("echo", &[]);
    exec.argv = (0..32)
        .map(|n| format!("{n:02}{}", "y".repeat(254)))
        .collect();
    exec.argv[0] = "echo".to_owned();
    let error = fx
        .guard
        .create(
            &ctx,
            create_with_secret(),
            &exec,
            None,
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap_err();
    let SandboxError::ApprovalRequired { subject: minimal } = error else {
        panic!("expected an approval request");
    };
    // Without an approval the guard answers with a minimal subject: no content at all.
    let text = serde_json::to_string(&minimal).unwrap();
    assert!(!text.contains(SECRET) && !text.contains("stdin-") && !text.contains("yyyy"));
    // The request filed with the approval engine shows the whole call.
    let subject = fx
        .guard
        .request_approval(&ctx, &create_with_secret(), &exec)
        .unwrap()
        .subject;
    let shown = &subject.0["exec"];
    let argv = shown["argv"].as_array().unwrap();
    assert_eq!(argv.len(), 32);
    assert_eq!(argv[1].as_str().unwrap().chars().count(), 256);
    assert_eq!(subject.0["create"]["env"][0]["name"], "API_KEY");
    let text = serde_json::to_string(&subject).unwrap();
    assert!(!text.contains(SECRET) && !text.contains("stdin-"), "{text}");
    assert_eq!(fx.sim.program_calls(), 0);
    assert!(!visible(&fx).contains(SECRET));
}

#[tokio::test]
async fn a_short_configured_retention_cannot_make_the_simulator_run_a_program_twice() {
    // A backend asked to forget after one second is built with the policy ceiling instead.
    let fx = fx();
    let short = SimBackend::with_parts(
        Arc::clone(&fx.clock) as Arc<dyn crate::clock::SandboxClock>,
        Arc::new(SimSecrets::new()),
        Arc::new(SimNetwork::new()),
        SimConfig {
            retention_ms: 1_000,
            ..SimConfig::default()
        },
    );
    assert!(short.retention_ms() >= crate::policy::MAX_RESULT_RETENTION_MS);
    short.register("count", |_| Ok(SimOutput::success()));
    let guard = restarted_over(&fx, short.clone());
    let ctx = ctx();
    let exec = exec_of("count", &[]);
    let request = guard.request_approval(&ctx, &create_req(), &exec).unwrap();
    let claim = ApprovalClaim {
        approval_id: "apr-short".to_owned(),
        call_key: request.call_key.clone(),
    };
    fx.verifier.approve(VerifiedGrant {
        approval_id: claim.approval_id.clone(),
        call_key: request.call_key,
        tenant_id: ctx.tenant_id.clone(),
        run_id: ctx.run_id.clone(),
        requested_by: "agent".to_owned(),
        approved_by: "alice".to_owned(),
        approval_payload_hash: "ab".repeat(32),
    });
    let handle = guard
        .create(
            &ctx,
            create_req(),
            &exec,
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    guard
        .exec(
            &ctx,
            &handle,
            exec.clone(),
            Some(&claim),
            RUN_STARTED,
            &never(),
        )
        .await
        .unwrap();
    guard.destroy(&ctx, &handle).await.unwrap();
    assert_eq!(short.program_calls(), 1);

    // Twice the configured retention later, and then past the guard's own, the same key
    // never runs the program again: it replays, or the guard refuses the old attempt.
    for advance in [2_000, 25 * 3_600_000] {
        fx.clock.advance(advance);
        let again = restarted_over(&fx, short.clone());
        let replayed = again
            .create(
                &ctx,
                create_req(),
                &exec,
                Some(&claim),
                RUN_STARTED,
                &never(),
            )
            .await;
        match replayed {
            Ok(h) => {
                let _ = again
                    .exec(&ctx, &h, exec.clone(), Some(&claim), RUN_STARTED, &never())
                    .await;
            }
            Err(error) => assert_eq!(error.full_code(), "policy_denied.attempt_too_old"),
        }
        assert_eq!(short.program_calls(), 1, "after {advance} ms");
    }
}
