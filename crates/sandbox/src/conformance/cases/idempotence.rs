//! Idempotence: the same key with the same request returns what was obtained, without a
//! second effect; the same key with another request is a conflict; and what the backend
//! keeps survives a restart of the host.

use super::{case, CaseDef};
use crate::cancel::CancelToken;
use crate::conformance::support::{
    ensure, expect_code, poll_once, shape, skip, BoxFut, Env, Lab, Must, Stop, Verdict,
};
use crate::conformance::target::MarkerState;
use crate::error::{SandboxError, SandboxState};
use crate::ids::SandboxStep;
use crate::outcome::ExecOutcome;
use crate::policy::{SandboxPolicy, MAX_RESULT_RETENTION_MS};
use crate::request::CollectRequest;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Idempotence,
            "idempotence.replay_without_backend_call",
            Guard,
            "a replayed create, exec and collect return the recorded result without the backend",
            replay_without_backend_call
        ),
        case!(
            Idempotence,
            "idempotence.conflict_in_every_state",
            Guard,
            "the same key with another request is an idempotency conflict in every state",
            conflict_in_every_state
        ),
        case!(
            Idempotence,
            "idempotence.backend_conflict_in_every_state",
            RealBackend,
            "the backend answers a conflict for the same key with another request, in every state",
            backend_conflict_in_every_state
        ),
        case!(
            Idempotence,
            "idempotence.concurrent_execs_one_call",
            Guard,
            "twenty concurrent execs with one key make one backend call, the others are busy",
            concurrent_execs_one_call
        ),
        case!(
            Idempotence,
            "idempotence.resume_after_cancellation",
            Guard,
            "an exec cancelled before or at the backend can be asked again with the same key",
            resume_after_cancellation
        ),
        case!(
            Idempotence,
            "idempotence.unknown_outcome_allows_only_the_same_exec_or_destroy",
            Guard,
            "while the outcome of an exec is unknown only the same exec or destroy is allowed",
            unknown_outcome_allows_only_the_same_exec_or_destroy
        ),
        case!(
            Idempotence,
            "idempotence.final_errors_not_asked_again",
            Guard,
            "an error that is not retriable is remembered and answered without the backend",
            final_errors_not_asked_again
        ),
        case!(
            Idempotence,
            "idempotence.retriable_errors_asked_again",
            Guard,
            "a wait, a retriable unavailability and a cancellation are asked again",
            retriable_errors_asked_again
        ),
        case!(
            Idempotence,
            "idempotence.restart_replay",
            RealBackend,
            "a restarted host replaying the node gets the stored results and no second run",
            restart_replay
        ),
        case!(
            Idempotence,
            "idempotence.replay_after_destroy",
            RealBackend,
            "the stored answers outlive destroy: a replay is served and runs nothing",
            replay_after_destroy
        ),
        case!(
            Idempotence,
            "idempotence.marker_without_result",
            SimulatorOnly,
            "a marker without a result is unavailable for good and the program is not run again",
            marker_without_result
        ),
        case!(
            Idempotence,
            "idempotence.retention_covers_the_ceiling",
            SimulatorOnly,
            "the backend keeps its answers for the longest retention a policy can ask for",
            retention_covers_the_ceiling
        ),
        case!(
            Idempotence,
            "idempotence.attempt_age",
            Guard,
            "an attempt too old or too far in the future is refused before any effect",
            attempt_age
        ),
    ]
}

async fn replay_without_backend_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let create = env.create_req();
    let (handle, claim) = env.ready(&ctx, create.clone(), &exec).await?;
    let outcome = env
        .run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    let collected = env.gather(&ctx, &handle).await.must("collect")?;
    let (calls, events) = (env.tap.total(), env.journal.len());

    let again = env
        .guard
        .create(
            &ctx,
            create,
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await
        .must("replayed create")?;
    ensure!(again == handle, "a replayed create returned another handle");
    let outcome_again = env
        .run(&ctx, &handle, exec, &claim)
        .await
        .must("replayed exec")?;
    ensure!(
        outcome_again == outcome,
        "a replayed exec returned another outcome"
    );
    let collected_again = env.gather(&ctx, &handle).await.must("replayed collect")?;
    ensure!(
        collected_again == collected,
        "a replayed collect returned another result"
    );
    ensure!(env.tap.total() == calls, "a replay reached the backend");
    ensure!(env.journal.len() == events, "a replay wrote an event again");
    Ok(())
}

/// An exec that differs from the one the key was used for.
struct Different<'a> {
    env: &'a Env,
    ctx: &'a crate::ids::SandboxContext,
    handle: &'a crate::ids::SandboxHandle,
    exec: crate::request::ExecRequest,
    claim: crate::ApprovalClaim,
}

impl Different<'_> {
    async fn conflicts(&self, state: &str) -> Verdict {
        let result = self
            .env
            .run(self.ctx, self.handle, self.exec.clone(), &self.claim)
            .await;
        expect_code(
            &result,
            "idempotency_conflict",
            &format!("a different exec {state}"),
        )
    }
}

async fn conflict_in_every_state(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");

    // A different create under the same key.
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let mut changed = env.create_req();
    changed.ttl_ms += 1;
    let changed_claim = env
        .approve(&ctx, &changed, &exec)
        .must("approval request")?;
    let result = env
        .guard
        .create(
            &ctx,
            changed,
            &exec,
            Some(&changed_claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(&result, "idempotency_conflict", "a different create")?;

    // A different exec, in each state that has an exec: unknown outcome, executed, collected,
    // destroyed.
    let mut other = exec.clone();
    other.argv.push("--other".to_owned());
    let other_claim = env
        .approve(&ctx, &env.create_req(), &other)
        .must("approval request")?;
    let different = Different {
        env: &env,
        ctx: &ctx,
        handle: &handle,
        exec: other,
        claim: other_claim,
    };
    env.tap.fail_next(
        SandboxStep::Exec,
        SandboxError::Unavailable { retriable: true },
    );
    let first = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        first.is_err(),
        "the scripted unavailability was not reported"
    );
    different.conflicts("when the outcome is unknown").await?;
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    different.conflicts("when executed").await?;
    env.gather(&ctx, &handle).await.must("collect")?;
    different.conflicts("when collected").await?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    different.conflicts("when destroyed").await?;
    // A different collect once collected: the sandbox is gone by now, so use a second one.
    let ctx2 = env.ctx("tenant-a", "node-2");
    let (second, claim2) = env.ready(&ctx2, env.create_req(), &exec).await?;
    env.run(&ctx2, &second, exec, &claim2).await.must("exec")?;
    env.gather(&ctx2, &second).await.must("collect")?;
    let different = CollectRequest {
        max_files: 2,
        ..env.collect_req()
    };
    let result = env
        .guard
        .collect(&ctx2, &second, different, env.run_started(), &Env::never())
        .await;
    expect_code(&result, "idempotency_conflict", "a different collect")?;
    // A call that is only out of order is not a conflict.
    let ctx3 = env.ctx("tenant-a", "node-3");
    let (third, _) = env
        .ready(&ctx3, env.create_req(), &Env::success(lab)?)
        .await?;
    let early = env.gather(&ctx3, &third).await;
    expect_code(&early, "invalid_state", "a collect before exec")
}

async fn backend_conflict_in_every_state(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let mut other_exec = exec.clone();
    other_exec.argv.push("--other".to_owned());
    let ctx = env.ctx("tenant-a", "node-1");
    let mut other_create = env.create_req();
    other_create.ttl_ms += 1;
    let other_collect = CollectRequest {
        patterns: vec!["*".to_owned()],
        ..env.collect_req()
    };
    let (handle, _) = env.b_create(&ctx, env.create_req()).await.must("create")?;
    let result = env.b_create(&ctx, other_create.clone()).await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different create when created",
    )?;

    env.b_exec(&ctx, &handle, exec).await.must("exec")?;
    let result = env.b_exec(&ctx, &handle, other_exec.clone()).await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different exec when executed",
    )?;

    env.b_collect(&ctx, &handle).await.must("collect")?;
    let result = env
        .b_collect_with(&ctx, &handle, other_collect.clone())
        .await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different collect when collected",
    )?;

    env.b_destroy(&ctx, &handle).await.must("destroy")?;
    let result = env.b_exec(&ctx, &handle, other_exec).await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different exec when destroyed",
    )?;
    let result = env.b_collect_with(&ctx, &handle, other_collect).await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different collect when destroyed",
    )?;
    let result = env.b_create(&ctx, other_create).await;
    expect_code(
        &result,
        "idempotency_conflict",
        "a different create when destroyed",
    )
}

async fn concurrent_execs_one_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let gate = env.tap.hold(SandboxStep::Exec);
    let mut calls: Vec<BoxFut<'_, Result<ExecOutcome, SandboxError>>> = (0..20)
        .map(|_| {
            Box::pin(env.run(&ctx, &handle, exec.clone(), &claim))
                as BoxFut<'_, Result<ExecOutcome, SandboxError>>
        })
        .collect();
    let mut waiting = Vec::new();
    let mut busy = 0;
    for (index, call) in calls.iter_mut().enumerate() {
        match poll_once(call) {
            None => waiting.push(index),
            Some(Err(SandboxError::Busy { .. })) => busy += 1,
            Some(other) => {
                let got = shape(&other);
                return Err(Stop::Fail(format!(
                    "a concurrent exec answered {got}, expected busy"
                )));
            }
        }
    }
    ensure!(
        waiting.len() == 1 && busy == 19,
        "{} execs reached the backend and {busy} were busy, expected 1 and 19",
        waiting.len()
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "more than one exec reached the backend"
    );
    gate.open();
    let first = poll_once(&mut calls[waiting[0]]);
    ensure!(
        matches!(first, Some(Ok(_))),
        "the exec that held the key did not finish"
    );
    let retried = env.run(&ctx, &handle, exec, &claim).await.must("a retry")?;
    ensure!(
        matches!(&first, Some(Ok(outcome)) if *outcome == retried),
        "a retry did not return the result of the one execution"
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the retry reached the backend"
    );
    if let Some(runs) = env.hooks.executions().await {
        ensure!(runs == 1, "the program ran {runs} times");
    }
    Ok(())
}

async fn resume_after_cancellation(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;

    // Cancelled before the guard asks the backend.
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let flag = Arc::new(AtomicBool::new(true));
    let seen = Arc::clone(&flag);
    let token = CancelToken::new(Arc::new(move || seen.load(Ordering::SeqCst)));
    let cancelled = env
        .guard
        .exec(
            &ctx,
            &handle,
            exec.clone(),
            Some(&claim),
            env.run_started(),
            &token,
        )
        .await;
    expect_code(&cancelled, "cancelled", "an exec with a cancelled token")?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "a cancelled exec reached the backend"
    );
    flag.store(false, Ordering::SeqCst);
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("the same exec after cancellation")?;

    // Cancelled at the backend.
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.tap
        .fail_next(SandboxStep::Exec, SandboxError::Cancelled);
    let cancelled = env.run(&ctx, &handle, exec.clone(), &claim).await;
    expect_code(
        &cancelled,
        "cancelled",
        "an exec the backend reports cancelled",
    )?;
    env.run(&ctx, &handle, exec, &claim)
        .await
        .must("the same exec after a cancellation at the backend")?;
    Ok(())
}

async fn unknown_outcome_allows_only_the_same_exec_or_destroy(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.tap.fail_next(
        SandboxStep::Exec,
        SandboxError::Unavailable { retriable: true },
    );
    let lost = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        matches!(&lost, Err(error) if error.is_retriable()),
        "the scripted unavailability was not reported as retriable"
    );
    let early = env.gather(&ctx, &handle).await;
    ensure!(
        matches!(
            &early,
            Err(SandboxError::InvalidState {
                from: SandboxState::Executing
            })
        ),
        "collect while the outcome is unknown: expected invalid_state(Executing), got {}",
        shape(&early)
    );
    let mut other = exec.clone();
    other.argv.push("--other".to_owned());
    let other_claim = env
        .approve(&ctx, &env.create_req(), &other)
        .must("approval request")?;
    let refused = env.run(&ctx, &handle, other, &other_claim).await;
    expect_code(
        &refused,
        "idempotency_conflict",
        "another exec while the outcome is unknown",
    )?;
    env.run(&ctx, &handle, exec, &claim)
        .await
        .must("the same exec")?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}

async fn final_errors_not_asked_again(lab: &Lab<'_>) -> Verdict {
    for terminal in [
        SandboxError::Unavailable { retriable: false },
        SandboxError::Backend("conformance".to_owned()),
    ] {
        let env = lab.env().await;
        let exec = Env::success(lab)?;
        let ctx = env.ctx("tenant-a", "node-1");
        let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
        env.tap.fail_next(SandboxStep::Exec, terminal);
        let first = env.run(&ctx, &handle, exec.clone(), &claim).await;
        let again = env.run(&ctx, &handle, exec, &claim).await;
        ensure!(first.is_err(), "the scripted error was not reported");
        ensure!(
            shape(&first) == shape(&again),
            "the error was {} then {}",
            shape(&first),
            shape(&again)
        );
        ensure!(
            env.tap.count(SandboxStep::Exec) == 1,
            "a final error was asked of the backend again"
        );
        env.guard
            .destroy(&ctx, &handle)
            .await
            .must("destroy after a final error")?;
    }
    Ok(())
}

async fn retriable_errors_asked_again(lab: &Lab<'_>) -> Verdict {
    for retry in [
        SandboxError::Unavailable { retriable: true },
        SandboxError::Busy {
            retry_after_ms: None,
        },
        SandboxError::Cancelled,
    ] {
        let env = lab.env().await;
        let exec = Env::success(lab)?;
        let ctx = env.ctx("tenant-a", "node-1");
        let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
        let name = retry.full_code();
        env.tap.fail_next(SandboxStep::Exec, retry);
        let first = env.run(&ctx, &handle, exec.clone(), &claim).await;
        ensure!(first.is_err(), "the scripted {name} was not reported");
        env.run(&ctx, &handle, exec, &claim)
            .await
            .must(&format!("the same exec after {name}"))?;
        ensure!(
            env.tap.count(SandboxStep::Exec) == 2,
            "{name} was not asked of the backend again"
        );
    }
    Ok(())
}

/// Run the node through `guard`: create, exec, collect. Returns what a node would keep.
async fn replay_node(
    env: &Env,
    guard: &crate::guard::GovernedSandbox<crate::conformance::support::Tap>,
    ctx: &crate::ids::SandboxContext,
    exec: &crate::request::ExecRequest,
    claim: &crate::ApprovalClaim,
) -> Result<(ExecOutcome, crate::collected::Collected), Stop> {
    let start = env.run_started();
    let handle = guard
        .create(
            ctx,
            env.create_req(),
            exec,
            Some(claim),
            start,
            &Env::never(),
        )
        .await
        .must("replayed create")?;
    let outcome = guard
        .exec(
            ctx,
            &handle,
            exec.clone(),
            Some(claim),
            start,
            &Env::never(),
        )
        .await
        .must("replayed exec")?;
    let collected = guard
        .collect(ctx, &handle, env.collect_req(), start, &Env::never())
        .await
        .must("replayed collect")?;
    Ok((outcome, collected))
}

async fn restart_replay(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let Some(before) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    let runs = env.hooks.executions().await.unwrap_or(before);
    ensure!(
        runs == before + 1,
        "the program ran {} times for one exec",
        runs - before
    );
    let events = env.journal.len();

    // The host restarts and replays the node: a new guard, the same backend.
    let guard = env.restarted();
    let (outcome, collected) = replay_node(&env, &guard, &ctx, &exec, &ran.claim).await?;
    ensure!(
        outcome == ran.outcome,
        "the replayed exec returned another outcome"
    );
    ensure!(
        collected == ran.collected,
        "the replayed collect returned another result"
    );
    let after = env.hooks.executions().await.unwrap_or(0);
    ensure!(
        after == runs,
        "the replay ran the program {} more times",
        after - runs
    );
    ensure!(
        env.journal.len() == events,
        "the replay wrote an event again"
    );
    Ok(())
}

async fn replay_after_destroy(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let Some(before) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    let guard = env.restarted();
    let (outcome, collected) = replay_node(&env, &guard, &ctx, &exec, &ran.claim).await?;
    ensure!(
        outcome == ran.outcome,
        "the exec replayed after destroy returned another outcome"
    );
    ensure!(
        collected == ran.collected,
        "the collect replayed after destroy returned another result"
    );
    let runs = env.hooks.executions().await.unwrap_or(0);
    ensure!(
        runs == before + 1,
        "the program ran {} times",
        runs - before
    );
    Ok(())
}

async fn marker_without_result(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    if !env.hooks.arm_crash_after_marker().await {
        skip!("the target cannot inject a host crash after the execution marker");
    }
    let Some(_) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    let crashed = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        crashed.is_err(),
        "an exec whose host crashed reported success"
    );
    let Some(marker) = env.hooks.marker(&ctx, SandboxStep::Exec).await else {
        skip!("the target cannot read the idempotency marker");
    };
    ensure!(
        marker == MarkerState::MarkerOnly,
        "after the crash the backend keeps {marker:?}, expected the marker alone"
    );
    let runs = env.hooks.executions().await.unwrap_or(0);
    let retried = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        matches!(
            &retried,
            Err(SandboxError::Unavailable { retriable: false })
        ),
        "the same exec after the crash: expected unavailable (not retriable), got {}",
        shape(&retried)
    );
    let after = env.hooks.executions().await.unwrap_or(0);
    ensure!(after == runs, "the program was run again after the crash");
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}

async fn retention_covers_the_ceiling(lab: &Lab<'_>) -> Verdict {
    let Ok(policy) = SandboxPolicy::default().with_result_retention_ms(MAX_RESULT_RETENTION_MS)
    else {
        skip!("the retention ceiling cannot be configured");
    };
    let env = lab.env_with(policy).await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let Some(before) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    if let Some(kept) = env.hooks.retention_ms().await {
        ensure!(
            kept >= MAX_RESULT_RETENTION_MS,
            "the backend keeps its answers {kept} ms, less than the longest retention a policy can ask for"
        );
    }
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    // Almost the whole retention later, the host replays the node.
    if !env
        .hooks
        .advance_clock(MAX_RESULT_RETENTION_MS - 60_000)
        .await
    {
        skip!("the target cannot move its clock by days");
    }
    let guard = env.restarted();
    let (outcome, collected) = replay_node(&env, &guard, &ctx, &exec, &ran.claim).await?;
    ensure!(
        outcome == ran.outcome,
        "the exec replayed late returned another outcome"
    );
    ensure!(
        collected == ran.collected,
        "the collect replayed late returned another result"
    );
    let runs = env.hooks.executions().await.unwrap_or(0);
    ensure!(
        runs == before + 1,
        "the program ran {} times",
        runs - before
    );
    Ok(())
}

async fn attempt_age(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let now = env.clock.now_ms();
    let retention = env.policy.result_retention_ms();
    let skew = env.policy.max_clock_skew_ms;
    let too_old = now.saturating_sub(retention + 1);
    let in_future = now + skew + 1;
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;

    for (started, wanted, what) in [
        (
            too_old,
            "policy_denied.attempt_too_old",
            "an attempt that started before the retention",
        ),
        (
            in_future,
            "policy_denied.attempt_in_future",
            "an attempt that starts in the future",
        ),
    ] {
        let result = env
            .guard
            .create(
                &ctx,
                env.create_req(),
                &exec,
                Some(&claim),
                started,
                &Env::never(),
            )
            .await;
        expect_code(&result, wanted, &format!("create: {what}"))?;
    }
    ensure!(env.tap.total() == 0, "a refused create reached the backend");

    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let calls = env.tap.total();
    for (started, wanted, what) in [
        (
            too_old,
            "policy_denied.attempt_too_old",
            "an attempt that started before the retention",
        ),
        (
            in_future,
            "policy_denied.attempt_in_future",
            "an attempt that starts in the future",
        ),
    ] {
        let result = env
            .guard
            .exec(
                &ctx,
                &handle,
                exec.clone(),
                Some(&claim),
                started,
                &Env::never(),
            )
            .await;
        expect_code(&result, wanted, &format!("exec: {what}"))?;
    }
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    let calls_after_exec = env.tap.total();
    ensure!(
        calls_after_exec == calls + 1,
        "a refused exec reached the backend"
    );
    for (started, wanted, what) in [
        (
            too_old,
            "policy_denied.attempt_too_old",
            "an attempt that started before the retention",
        ),
        (
            in_future,
            "policy_denied.attempt_in_future",
            "an attempt that starts in the future",
        ),
    ] {
        let result = env
            .guard
            .collect(&ctx, &handle, env.collect_req(), started, &Env::never())
            .await;
        expect_code(&result, wanted, &format!("collect: {what}"))?;
    }
    ensure!(
        env.tap.total() == calls_after_exec,
        "a refused collect reached the backend"
    );
    Ok(())
}
