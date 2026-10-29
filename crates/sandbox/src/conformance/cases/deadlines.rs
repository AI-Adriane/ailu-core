//! The attempt deadline: the last instant at which the host may ask again for an attempt. The
//! guard hands it to the backend with every `create`, `exec` and `collect`; the backend seals it
//! with the key, keeps the key at least until that instant included, and refuses a deadline it
//! could not keep its promise for. The declared `max_retention` is the bound of that promise.

use super::{case, CaseDef};
use crate::backend::SandboxBackend;
use crate::conformance::support::{ensure, expect_code, shape, skip, Env, Lab, Must, Verdict};
use crate::ids::SandboxStep;
use crate::policy::{SandboxPolicy, MAX_RESULT_RETENTION_MS};

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Idempotence,
            "idempotence.guard_hands_the_sealed_deadline",
            Guard,
            "the backend is given the start of the run plus the retention recorded then, whatever the retention is later",
            guard_hands_the_sealed_deadline
        ),
        case!(
            Idempotence,
            "idempotence.max_retention_declared",
            RealBackend,
            "the backend declares a retention, keeps at least that long and does not change it while it runs",
            max_retention_declared
        ),
        case!(
            Idempotence,
            "idempotence.deadline_sealed_at_first_call",
            RealBackend,
            "a repeated key is served with its sealed deadline and refused with a later one, at create, exec and collect",
            deadline_sealed_at_first_call
        ),
        case!(
            Idempotence,
            "idempotence.deadline_beyond_max_retention_refused",
            RealBackend,
            "a deadline later than now plus max_retention is refused when the key is sealed, without effect",
            deadline_beyond_max_retention_refused
        ),
        case!(
            Idempotence,
            "idempotence.key_kept_through_the_deadline",
            RealBackend,
            "a replay at the very instant of the deadline is served, and one instant later the guard refuses it",
            key_kept_through_the_deadline
        ),
    ]
}

/// A retention the case can ask a guard for: what the backend declares, up to the ceiling.
fn usable_retention(declared: u64) -> u64 {
    declared.min(MAX_RESULT_RETENTION_MS)
}

async fn guard_hands_the_sealed_deadline(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let create = env.create_req();
    let start = env.run_started();
    let window = env.guard.attempt_window(start);
    let retention = env.policy.effective_retention_ms(env.tap.max_retention());
    let sealed = start + retention;
    let claim = env.approve(&ctx, &create, &exec).must("approval request")?;
    let never = Env::never();
    let handle = env
        .guard
        .create(&ctx, create.clone(), &exec, Some(&claim), window, &never)
        .await
        .must("create")?;
    env.guard
        .exec(&ctx, &handle, exec.clone(), Some(&claim), window, &never)
        .await
        .must("exec")?;
    env.guard
        .collect(&ctx, &handle, env.collect_req(), window, &never)
        .await
        .must("collect")?;
    for step in [SandboxStep::Create, SandboxStep::Exec, SandboxStep::Collect] {
        ensure!(
            env.tap.deadlines(step) == [sealed],
            "the deadline given to the backend for {step:?} is not the start of the run plus the retention of that instant"
        );
    }

    // The host restarts with a longer retention and replays the node with the window it kept:
    // the deadline does not move, the backend still keeps its state for the window it was
    // promised and no further.
    let longer = (retention.saturating_mul(2)).min(usable_retention(env.tap.max_retention()));
    if longer > retention {
        let Ok(policy) = SandboxPolicy::default().with_result_retention_ms(longer) else {
            skip!("the longer retention cannot be configured");
        };
        let restarted = env.restarted_under(policy);
        let replayed = restarted
            .create(&ctx, create, &exec, Some(&claim), window, &never)
            .await
            .must("replayed create")?;
        ensure!(
            replayed.sandbox_id() == handle.sandbox_id(),
            "the replayed create returned another sandbox"
        );
        ensure!(
            env.tap.deadlines(SandboxStep::Create) == [sealed, sealed],
            "a retention raised later moved the deadline of a replay"
        );
        // A bare start instant records nothing: it is judged on the retention in force now.
        let bare = restarted
            .create(
                &env.ctx("tenant-a", "node-2"),
                env.create_req(),
                &exec,
                Some(
                    &env.approve(&env.ctx("tenant-a", "node-2"), &env.create_req(), &exec)
                        .must("approval request")?,
                ),
                start,
                &never,
            )
            .await;
        ensure!(bare.is_ok(), "a create with a bare start: {}", shape(&bare));
        ensure!(
            env.tap.deadlines(SandboxStep::Create).last() == Some(&(start + longer)),
            "a bare start instant is not judged on the retention in force"
        );
    }
    Ok(())
}

async fn max_retention_declared(lab: &Lab<'_>) -> Verdict {
    let declared = lab.max_retention().await;
    ensure!(declared > 0, "the backend declares no retention");
    let env = lab.env().await;
    ensure!(
        env.tap.max_retention() == declared,
        "the backend declares another retention from one instance to the next"
    );
    if let Some(kept) = env.hooks.retention_ms().await {
        ensure!(
            kept >= declared,
            "the backend keeps its answers {kept} ms and declares {declared} ms"
        );
    }
    // What it declares does not change while it works, nor as time goes by.
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    ensure!(
        env.tap.max_retention() == declared,
        "the declared retention changed after a run"
    );
    if env.hooks.advance_clock(24 * 3600 * 1000).await {
        ensure!(
            env.tap.max_retention() == declared,
            "the declared retention changed as time went by"
        );
    }
    Ok(())
}

/// How much later than the sealed one the deadline of a repeat is: one millisecond is enough,
/// and it stays far below the limit of `max_retention`.
const LATER_BY: u64 = 1;

async fn deadline_sealed_at_first_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let sealed = env.deadline();
    let later = sealed + LATER_BY;
    let never = Env::never();

    // create
    let (handle, raw) = env
        .b_create_at(&ctx, env.create_req(), sealed, &never)
        .await
        .must("create")?;
    let again = env
        .b_create_at(&ctx, env.create_req(), sealed, &never)
        .await
        .must("the same create with the sealed deadline")?;
    ensure!(
        again.1 == raw,
        "the same create returned another result with the sealed deadline"
    );
    let refused = env.b_create_at(&ctx, env.create_req(), later, &never).await;
    ensure!(
        refused.is_err(),
        "a create repeated with a later deadline than the one it sealed was served"
    );

    // exec
    let before = env.hooks.executions().await;
    let outcome = env
        .b_exec_at(&ctx, &handle, exec.clone(), sealed, &never)
        .await
        .must("exec")?;
    let served = env
        .b_exec_at(&ctx, &handle, exec.clone(), sealed, &never)
        .await
        .must("the same exec with the sealed deadline")?;
    ensure!(
        served == outcome,
        "the same exec returned another outcome with the sealed deadline"
    );
    let refused = env
        .b_exec_at(&ctx, &handle, exec.clone(), later, &never)
        .await;
    ensure!(
        refused.is_err(),
        "an exec repeated with a later deadline than the one it sealed was served"
    );
    if let (Some(before), Some(after)) = (before, env.hooks.executions().await) {
        ensure!(
            after == before + 1,
            "the program ran {} times",
            after - before
        );
    }

    // collect
    let collected = env
        .b_collect_at(&ctx, &handle, env.collect_req(), sealed, &never)
        .await
        .must("collect")?;
    let served = env
        .b_collect_at(&ctx, &handle, env.collect_req(), sealed, &never)
        .await
        .must("the same collect with the sealed deadline")?;
    ensure!(
        served.stdout == collected.stdout && served.files.len() == collected.files.len(),
        "the same collect returned another result with the sealed deadline"
    );
    let refused = env
        .b_collect_at(&ctx, &handle, env.collect_req(), later, &never)
        .await;
    ensure!(
        refused.is_err(),
        "a collect repeated with a later deadline than the one it sealed was served"
    );
    // The refusals left the sealed answers where they were.
    env.b_exec_at(&ctx, &handle, exec, sealed, &never)
        .await
        .must("the exec again after the refusals")?;
    Ok(())
}

async fn deadline_beyond_max_retention_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let max = env.tap.max_retention();
    let now = env.clock.now_ms();
    let Some(limit) = now.checked_add(max) else {
        skip!("the backend declares a retention no deadline can go past");
    };
    // A margin that no read of the clock between here and the backend can eat.
    let Some(beyond) = limit.checked_add(60_000) else {
        skip!("the backend declares a retention no deadline can go past");
    };
    let never = Env::never();

    // create: refused, and nothing was reserved.
    let refused = env
        .b_create_at(&ctx, env.create_req(), beyond, &never)
        .await;
    ensure!(
        refused.is_err(),
        "a create sealed with a deadline beyond max_retention was accepted"
    );
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(live == 0, "a refused create left {live} resources");
    }
    // The deadline at the limit is the longest it keeps: accepted.
    let (handle, _) = env
        .b_create_at(&ctx, env.create_req(), limit, &never)
        .await
        .must("a create sealed at now plus max_retention")?;

    // exec: refused, and the program did not run.
    let before = env.hooks.executions().await;
    let refused = env
        .b_exec_at(&ctx, &handle, exec.clone(), beyond, &never)
        .await;
    ensure!(
        refused.is_err(),
        "an exec sealed with a deadline beyond max_retention was accepted"
    );
    if let (Some(before), Some(after)) = (before, env.hooks.executions().await) {
        ensure!(after == before, "a refused exec ran the program");
    }
    env.b_exec_at(&ctx, &handle, exec, limit, &never)
        .await
        .must("an exec sealed at now plus max_retention")?;

    // collect: refused, and the sandbox is still collectable.
    let refused = env
        .b_collect_at(&ctx, &handle, env.collect_req(), beyond, &never)
        .await;
    ensure!(
        refused.is_err(),
        "a collect sealed with a deadline beyond max_retention was accepted"
    );
    env.b_collect_at(&ctx, &handle, env.collect_req(), limit, &never)
        .await
        .must("a collect sealed at now plus max_retention")?;
    Ok(())
}

async fn key_kept_through_the_deadline(lab: &Lab<'_>) -> Verdict {
    let declared = lab.max_retention().await;
    let retention = usable_retention(declared);
    if retention < 4_000 {
        skip!("the backend keeps its keys too briefly to move a clock to their deadline");
    }
    let Ok(policy) = SandboxPolicy::default().with_result_retention_ms(retention) else {
        skip!("the retention the backend declares cannot be configured");
    };
    let env = lab.env_with(policy).await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let Some(before) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    // The run starts now: the longest deadline the guard gives is the limit of what the
    // backend promised.
    let start = env.clock.now_ms();
    let window = env.guard.attempt_window(start);
    let deadline = start + retention;
    let never = Env::never();
    let create = env.create_req();
    let claim = env.approve(&ctx, &create, &exec).must("approval request")?;
    let handle = env
        .guard
        .create(&ctx, create.clone(), &exec, Some(&claim), window, &never)
        .await
        .must("create")?;
    let outcome = env
        .guard
        .exec(&ctx, &handle, exec.clone(), Some(&claim), window, &never)
        .await
        .must("exec")?;
    let collected = env
        .guard
        .collect(&ctx, &handle, env.collect_req(), window, &never)
        .await
        .must("collect")?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;

    let wait = deadline.saturating_sub(env.clock.now_ms());
    if !env.hooks.advance_clock(wait).await {
        skip!("the target cannot move its clock to the deadline");
    }
    let now = env.clock.now_ms();
    if now != deadline {
        skip!("the clock of the target cannot be stopped at the deadline");
    }
    // The host restarts at the very instant of the deadline and replays the node: the guard
    // still accepts the attempt, and every key is still there.
    let restarted = env.restarted();
    let replayed = restarted
        .create(&ctx, create.clone(), &exec, Some(&claim), window, &never)
        .await
        .must("create replayed at the deadline")?;
    ensure!(
        replayed.sandbox_id() == handle.sandbox_id(),
        "the create replayed at the deadline returned another sandbox"
    );
    let replayed_outcome = restarted
        .exec(&ctx, &replayed, exec.clone(), Some(&claim), window, &never)
        .await
        .must("exec replayed at the deadline")?;
    ensure!(
        replayed_outcome == outcome,
        "the exec replayed at the deadline returned another outcome"
    );
    let replayed_collected = restarted
        .collect(&ctx, &replayed, env.collect_req(), window, &never)
        .await
        .must("collect replayed at the deadline")?;
    ensure!(
        replayed_collected == collected,
        "the collect replayed at the deadline returned another result"
    );
    let runs = env.hooks.executions().await.unwrap_or(0);
    ensure!(
        runs == before + 1,
        "the program ran {} times",
        runs.saturating_sub(before)
    );

    // One instant later the guard refuses the attempt itself, before the backend.
    if !env.hooks.advance_clock(1).await {
        skip!("the target cannot move its clock past the deadline");
    }
    let calls = env.tap.total();
    let late = restarted
        .create(&ctx, create, &exec, Some(&claim), window, &never)
        .await;
    expect_code(
        &late,
        "policy_denied.attempt_too_old",
        "a create one instant after the deadline",
    )?;
    ensure!(
        env.tap.total() == calls,
        "an attempt past its deadline reached the backend"
    );
    Ok(())
}
