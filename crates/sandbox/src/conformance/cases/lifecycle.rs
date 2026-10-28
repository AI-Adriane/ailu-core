//! Lifecycle: the order of the states, what is refused out of order, destruction, and what a
//! backend leaves behind.

use super::{case, CaseDef};
use crate::conformance::support::{
    ensure, expect_code, shape, skip, until, BoxFut, Env, Lab, Must, Verdict,
};
use crate::error::{QuotaKind, SandboxError, SandboxState};
use crate::ids::SandboxStep;
use crate::journal::SandboxEventKind;
use crate::outcome::DeclaredExit;
use crate::policy::SandboxPolicy;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Lifecycle,
            "lifecycle.state_order",
            Guard,
            "create, exec, collect and destroy reach the backend in this order, one event each",
            state_order
        ),
        case!(
            Lifecycle,
            "lifecycle.out_of_order",
            Guard,
            "an operation out of order is an invalid state, whatever the state it finds",
            out_of_order
        ),
        case!(
            Lifecycle,
            "lifecycle.second_exec_refused",
            Guard,
            "a second exec on the sandbox is refused and never reaches the backend",
            second_exec_refused
        ),
        case!(
            Lifecycle,
            "lifecycle.poisoned_accepts_only_destroy",
            Guard,
            "after an isolation violation the sandbox accepts only destroy",
            poisoned_accepts_only_destroy
        ),
        case!(
            Lifecycle,
            "lifecycle.destroyed_is_terminal",
            Guard,
            "a destroyed sandbox stays destroyed, even when an exec returns after the destroy",
            destroyed_is_terminal
        ),
        case!(
            Lifecycle,
            "lifecycle.slot_returned_once",
            Guard,
            "the tenant's slot goes back once, however many times destroy is called",
            slot_returned_once
        ),
        case!(
            Lifecycle,
            "lifecycle.slot_returned_once_after_ttl",
            Guard,
            "the slot of a sandbox whose ttl passed goes back once, even if it is destroyed afterwards",
            slot_returned_once_after_ttl
        ),
        case!(
            Lifecycle,
            "lifecycle.abandoned_call_frees_slot",
            Guard,
            "a create that is dropped mid-call gives its slot back",
            abandoned_call_frees_slot
        ),
        case!(
            Lifecycle,
            "lifecycle.registry_bounded",
            Guard,
            "the registry refuses past its bound, and a destroyed sandbox no longer counts",
            registry_bounded
        ),
        case!(
            Lifecycle,
            "lifecycle.backend_second_exec",
            RealBackend,
            "the backend runs one exec per sandbox: another key finds it already executed",
            backend_second_exec
        ),
        case!(
            Lifecycle,
            "lifecycle.backend_second_collect",
            RealBackend,
            "a second collect under another key is an invalid state, the same key returns the record",
            backend_second_collect
        ),
        case!(
            Lifecycle,
            "lifecycle.destroy_idempotent",
            RealBackend,
            "destroying a sandbox twice succeeds",
            destroy_idempotent
        ),
        case!(
            Lifecycle,
            "lifecycle.no_residue_after_destroy",
            RealBackend,
            "after destroy the backend holds no resource of the sandbox",
            no_residue_after_destroy
        ),
        case!(
            Lifecycle,
            "lifecycle.ttl_sweep",
            RealBackend,
            "a sandbox nobody destroys is swept when its ttl passes, and its slot comes back",
            ttl_sweep
        ),
    ]
}

fn state_is<T>(
    result: &Result<T, SandboxError>,
    wanted: SandboxState,
    what: &str,
) -> Result<(), crate::conformance::support::Stop> {
    ensure!(
        matches!(result, Err(SandboxError::InvalidState { from }) if *from == wanted),
        "{what}: expected invalid_state({wanted:?}), got {}",
        shape(result)
    );
    Ok(())
}

async fn state_order(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    ensure!(
        matches!(ran.outcome.declared_exit, DeclaredExit::Code { code: 0 }),
        "the plain command did not exit with status 0"
    );
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    ensure!(
        env.tap.steps()
            == [
                SandboxStep::Create,
                SandboxStep::Exec,
                SandboxStep::Collect,
                SandboxStep::Destroy
            ],
        "the backend was not asked create, exec, collect, destroy in this order"
    );
    ensure!(
        env.kinds()
            == [
                SandboxEventKind::SandboxCreated,
                SandboxEventKind::SandboxExecStarted,
                SandboxEventKind::SandboxExecCompleted,
                SandboxEventKind::SandboxCollected,
                SandboxEventKind::SandboxDestroyed
            ],
        "the journal does not hold one event per transition, in order"
    );
    ensure!(
        ran.handle.tenant_id() == &ctx.tenant_id,
        "the handle does not carry the tenant of the context"
    );
    Ok(())
}

async fn out_of_order(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    state_is(
        &env.gather(&ctx, &handle).await,
        SandboxState::Created,
        "collect before exec",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Collect) == 0,
        "a collect before exec reached the backend"
    );
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    env.gather(&ctx, &handle).await.must("collect")?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    state_is(
        &env.run(&ctx, &handle, exec, &claim).await,
        SandboxState::Destroyed,
        "exec after destroy",
    )?;
    state_is(
        &env.gather(&ctx, &handle).await,
        SandboxState::Destroyed,
        "collect after destroy",
    )
}

async fn second_exec_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    let mut second = exec.clone();
    second.argv.push("--again".to_owned());
    let second_claim = env
        .approve(&ctx, &env.create_req(), &second)
        .must("approval request")?;
    let refused = env.run(&ctx, &handle, second, &second_claim).await;
    expect_code(&refused, "idempotency_conflict", "a second, different exec")?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the second exec reached the backend"
    );
    ensure!(
        env.kinds().contains(&SandboxEventKind::SandboxDenied),
        "the refusal of the second exec is not in the journal"
    );
    Ok(())
}

async fn poisoned_accepts_only_destroy(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.tap.rewrite_exec(Box::new(|_| {
        Err(SandboxError::IsolationViolation {
            signal: "conformance".to_owned(),
        })
    }));
    let violated = env.run(&ctx, &handle, exec.clone(), &claim).await;
    expect_code(
        &violated,
        "isolation_violation",
        "an exec the backend reports as a violation",
    )?;
    state_is(
        &env.gather(&ctx, &handle).await,
        SandboxState::Poisoned,
        "collect when poisoned",
    )?;
    state_is(
        &env.run(&ctx, &handle, exec.clone(), &claim).await,
        SandboxState::Poisoned,
        "exec when poisoned",
    )?;
    env.guard
        .destroy(&ctx, &handle)
        .await
        .must("destroy of a poisoned sandbox")?;
    ensure!(
        env.journal
            .events()
            .iter()
            .any(|e| e.kind == SandboxEventKind::SandboxExecFailed
                && e.code.as_deref() == Some("isolation_violation")),
        "the violation is not in the journal"
    );

    // A violation the backend reports at collect poisons the sandbox too.
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    env.tap.fail_next(
        SandboxStep::Collect,
        SandboxError::IsolationViolation {
            signal: "conformance".to_owned(),
        },
    );
    let violated = env.gather(&ctx, &handle).await;
    expect_code(
        &violated,
        "isolation_violation",
        "a collect the backend reports as a violation",
    )?;
    let calls = env.tap.count(SandboxStep::Collect);
    state_is(
        &env.gather(&ctx, &handle).await,
        SandboxState::Poisoned,
        "collect after a violation at collect",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Collect) == calls,
        "a poisoned sandbox was collected from"
    );
    env.guard
        .destroy(&ctx, &handle)
        .await
        .must("destroy of a poisoned sandbox")?;
    Ok(())
}

fn quota(max_concurrent: u32) -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota.max_concurrent = max_concurrent;
    policy
}

/// How many more sandboxes `tenant` can create, up to `limit`.
async fn headroom(
    env: &Env,
    lab: &Lab<'_>,
    tenant: &str,
    limit: usize,
) -> Result<usize, crate::conformance::support::Stop> {
    let exec = Env::success(lab)?;
    let mut made = 0;
    for n in 0..limit {
        let ctx = env.ctx(tenant, &format!("headroom-{n}"));
        let claim = env
            .approve(&ctx, &env.create_req(), &exec)
            .must("approval request")?;
        match env
            .guard
            .create(
                &ctx,
                env.create_req(),
                &exec,
                Some(&claim),
                env.run_started(),
                &Env::never(),
            )
            .await
        {
            Ok(_) => made += 1,
            Err(SandboxError::QuotaExceeded { .. }) => break,
            Err(other) => {
                return Err(crate::conformance::support::Stop::Fail(format!(
                    "create refused with {}",
                    other.full_code()
                )))
            }
        }
    }
    Ok(made)
}

async fn destroyed_is_terminal(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(quota(2)).await;
    let exec = Env::success(lab)?;
    let other = env.ctx("tenant-a", "other");
    env.ready(&other, env.create_req(), &exec).await?;
    let ctx = env.ctx("tenant-a", "racing");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    // The exec has run at the backend, and its answer is still on the way when the destroy runs.
    let gate = env.tap.hold_after(SandboxStep::Exec);
    let mut late: BoxFut<'_, _> = Box::pin(env.run(&ctx, &handle, exec.clone(), &claim));
    let held = until(&mut late, || gate.arrivals() >= 1).await;
    ensure!(
        held.is_none(),
        "the exec did not get to the end of its backend call"
    );
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    gate.open();
    // The late answer comes back; whatever it is, it brings nothing back to life.
    let _ = (&mut late).await;
    state_is(
        &env.gather(&ctx, &handle).await,
        SandboxState::Destroyed,
        "collect after a late exec",
    )?;
    let destroys = env.tap.count(SandboxStep::Destroy);
    env.guard
        .destroy(&ctx, &handle)
        .await
        .must("second destroy")?;
    ensure!(
        env.tap.count(SandboxStep::Destroy) == destroys,
        "a destroyed sandbox was destroyed again at the backend"
    );
    // The slot went back exactly once: `other` still holds one of the two.
    let more = headroom(&env, lab, "tenant-a", 4).await?;
    ensure!(more == 1, "{more} sandboxes could be created, expected 1");
    Ok(())
}

async fn slot_returned_once(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(quota(2)).await;
    let exec = Env::success(lab)?;
    let held = env.ctx("tenant-a", "held");
    env.ready(&held, env.create_req(), &exec).await?;
    let gone = env.ctx("tenant-a", "gone");
    let (handle, _) = env.ready(&gone, env.create_req(), &exec).await?;
    for _ in 0..3 {
        env.guard.destroy(&gone, &handle).await.must("destroy")?;
    }
    let more = headroom(&env, lab, "tenant-a", 4).await?;
    ensure!(
        more == 1,
        "{more} sandboxes could be created after a repeated destroy, expected 1"
    );

    Ok(())
}

/// Needs the clock of the instance to move past a ttl; not applicable otherwise.
async fn slot_returned_once_after_ttl(lab: &Lab<'_>) -> Verdict {
    let exec = Env::success(lab)?;
    // A sandbox that expired gives its slot back when its ttl passes; destroying it after that
    // must not give it back a second time.
    let env = lab.env_with(quota(2)).await;
    env.ready(&env.ctx("tenant-a", "held"), env.create_req(), &exec)
        .await?;
    let mut short = env.create_req();
    short.ttl_ms = 1_000;
    let expiring = env.ctx("tenant-a", "expiring");
    let (handle, _) = env.ready(&expiring, short, &exec).await?;
    if !env.hooks.advance_clock(1_001).await {
        skip!("the target cannot move its clock past a ttl");
    }
    // Taking the slot the expiry freed fills the tenant's ceiling again.
    env.ready(
        &env.ctx("tenant-a", "after-the-ttl"),
        env.create_req(),
        &exec,
    )
    .await?;
    env.guard
        .destroy(&expiring, &handle)
        .await
        .must("destroy of an expired sandbox")?;
    let more = headroom(&env, lab, "tenant-a", 4).await?;
    ensure!(
        more == 0,
        "{more} sandboxes could be created, the slot of an expired sandbox went back twice"
    );
    Ok(())
}

async fn abandoned_call_frees_slot(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(quota(1)).await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "abandoned");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let gate = env.tap.hold(SandboxStep::Create);
    let never = Env::never();
    let mut call: BoxFut<'_, _> = Box::pin(env.guard.create(
        &ctx,
        env.create_req(),
        &exec,
        Some(&claim),
        env.run_started(),
        &never,
    ));
    let held = until(&mut call, || gate.arrivals() >= 1).await;
    ensure!(held.is_none(), "the create did not wait at the backend");
    drop(call);
    gate.open();
    let more = headroom(&env, lab, "tenant-a", 3).await?;
    ensure!(
        more == 1,
        "{more} sandboxes could be created after an abandoned create, expected 1"
    );
    Ok(())
}

async fn registry_bounded(lab: &Lab<'_>) -> Verdict {
    let mut policy = SandboxPolicy::default();
    policy.max_registry_records = 2;
    let env = lab.env_with(policy).await;
    let exec = Env::success(lab)?;
    let first = env.ctx("tenant-a", "n1");
    let (handle, _) = env.ready(&first, env.create_req(), &exec).await?;
    env.ready(&env.ctx("tenant-a", "n2"), env.create_req(), &exec)
        .await?;
    let third = env.ctx("tenant-a", "n3");
    let claim = env
        .approve(&third, &env.create_req(), &exec)
        .must("approval request")?;
    let refused = env
        .guard
        .create(
            &third,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    ensure!(
        matches!(
            &refused,
            Err(SandboxError::QuotaExceeded {
                kind: QuotaKind::RegistryFull
            })
        ),
        "a create past the registry bound: expected quota_exceeded.registry_full, got {}",
        shape(&refused)
    );
    ensure!(
        env.tap.count(SandboxStep::Create) == 2,
        "the refused create reached the backend"
    );
    env.guard.destroy(&first, &handle).await.must("destroy")?;
    env.guard
        .create(
            &third,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await
        .must("create after a destroy made room")?;
    Ok(())
}

async fn backend_second_exec(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.b_create(&ctx, env.create_req()).await.must("create")?;
    env.b_exec(&ctx, &handle, exec.clone()).await.must("exec")?;
    let again = env.b_exec(&Env::next_attempt(&ctx), &handle, exec).await;
    state_is(
        &again,
        SandboxState::Executed,
        "a second exec under another key",
    )
}

async fn backend_second_collect(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.b_create(&ctx, env.create_req()).await.must("create")?;
    env.b_exec(&ctx, &handle, exec).await.must("exec")?;
    let first = env.b_collect(&ctx, &handle).await.must("collect")?;
    let same = env
        .b_collect(&ctx, &handle)
        .await
        .must("the same collect again")?;
    ensure!(
        same.stdout == first.stdout && same.files.len() == first.files.len(),
        "the same collect again did not return the recorded result"
    );
    let other = env.b_collect(&Env::next_attempt(&ctx), &handle).await;
    state_is(
        &other,
        SandboxState::Collected,
        "a second collect under another key",
    )
}

async fn destroy_idempotent(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.b_create(&ctx, env.create_req()).await.must("create")?;
    env.b_exec(&ctx, &handle, exec).await.must("exec")?;
    env.b_destroy(&ctx, &handle).await.must("destroy")?;
    env.b_destroy(&ctx, &handle).await.must("second destroy")?;
    // Through the guard too.
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, _) = env
        .ready(&ctx, env.create_req(), &Env::success(lab)?)
        .await?;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    env.guard
        .destroy(&ctx, &handle)
        .await
        .must("second destroy")?;
    Ok(())
}

async fn no_residue_after_destroy(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    let Some(live) = env.hooks.live_resources().await else {
        skip!("the target cannot count the resources it holds");
    };
    ensure!(live == 0, "{live} resources remain after destroy");
    Ok(())
}

async fn ttl_sweep(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(quota(1)).await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "left-behind");
    let mut create = env.create_req();
    create.ttl_ms = 1_000;
    env.ready(&ctx, create, &exec).await?;
    // The host crashed: nobody destroys the sandbox.
    if !env.hooks.advance_clock(1_001).await {
        skip!("the target cannot move its clock past a ttl");
    }
    let Some(residue) = env.hooks.residue_after_sweep().await else {
        skip!("the target cannot count what remains after a sweep");
    };
    ensure!(
        residue == 0,
        "{residue} resources remain after the ttl and the sweep"
    );
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(live == 0, "{live} sandboxes are still held after their ttl");
    }
    // The guard gives the slot back at the same instant.
    let again = env.ctx("tenant-a", "after-the-ttl");
    env.ready(&again, env.create_req(), &exec).await?;
    Ok(())
}
