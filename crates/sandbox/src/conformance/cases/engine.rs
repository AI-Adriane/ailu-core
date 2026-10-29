//! Engine invariants: nothing the guard hands to a node names the sandbox (so nothing of it can
//! end up in a checkpoint), no sandbox is held while a node is suspended, and a cancellation is
//! observed at the boundaries with the sandbox destroyed in every case. The node here is the
//! one the guard is made for: it creates a sandbox with `with_sandbox`, runs one program,
//! collects, and keeps only digests. What a host puts in its own state is the host's business
//! and is not judged here.

use super::{case, CaseDef};
use crate::cancel::CancelToken;
use crate::conformance::support::Tap;
use crate::conformance::support::{ensure, expect_code, Env, Lab, Must, Verdict};
use crate::error::SandboxError;
use crate::guard::GovernedSandbox;
use crate::ids::{SandboxContext, SandboxStep};
use crate::journal::SandboxEventKind;
use crate::policy::SandboxPolicy;
use crate::request::ExecRequest;
use crate::ApprovalClaim;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Engine,
            "engine.guard_outputs_name_no_sandbox",
            Guard,
            "nothing the guard hands to a node names the sandbox, its domain or a key, and the node run again on the same attempt gives the same digests",
            guard_outputs_name_no_sandbox
        ),
        case!(
            Engine,
            "engine.no_sandbox_held_while_suspended",
            Guard,
            "a node that suspends on the approval holds no sandbox, and its resume runs once",
            no_sandbox_held_while_suspended
        ),
        case!(
            Engine,
            "engine.cancellation_at_boundaries",
            Guard,
            "a cancellation is observed before create, exec and collect, and the sandbox is destroyed",
            cancellation_at_boundaries
        ),
        case!(
            Engine,
            "engine.sandbox_destroyed_on_every_exit",
            Guard,
            "the sandbox of a node is destroyed whether the node succeeds, fails or is refused",
            sandbox_destroyed_on_every_exit
        ),
    ]
}

type Step = Option<Arc<dyn Fn() + Send + Sync>>;

/// What a node does between its calls, to let a case act at a boundary.
#[derive(Default)]
struct Between {
    before_exec: Step,
    before_collect: Step,
}

/// The node: one sandbox for the length of the node, one program, and a state made of digests.
/// `claims` are the approvals presented at create and at exec.
async fn node_on(
    env: &Env,
    guard: &GovernedSandbox<Tap>,
    ctx: &SandboxContext,
    exec: &ExecRequest,
    claims: (Option<&ApprovalClaim>, Option<&ApprovalClaim>),
    cancel: &CancelToken,
    between: Between,
) -> Result<Value, SandboxError> {
    let (create_claim, exec_claim) = claims;
    let exec_claim = exec_claim.cloned();
    let program = exec.clone();
    let started = env.run_started();
    let collect = env.collect_req();
    let collected = guard
        .with_sandbox(
            ctx,
            env.create_req(),
            exec,
            create_claim,
            started,
            cancel,
            move |session| {
                Box::pin(async move {
                    if let Some(act) = &between.before_exec {
                        act();
                    }
                    session.exec(program, exec_claim.as_ref(), started).await?;
                    if let Some(act) = &between.before_collect {
                        act();
                    }
                    session.collect(collect, started).await
                })
            },
        )
        .await?;
    Ok(json!({
        "outputsDigest": collected.outputs_digest,
        "stdoutSha256": collected.stdout_sha256,
        "stderrSha256": collected.stderr_sha256,
    }))
}

/// The node on the environment's own guard.
async fn node(
    env: &Env,
    ctx: &SandboxContext,
    exec: &ExecRequest,
    claims: (Option<&ApprovalClaim>, Option<&ApprovalClaim>),
    cancel: &CancelToken,
    between: Between,
) -> Result<Value, SandboxError> {
    node_on(env, &env.guard, ctx, exec, claims, cancel, between).await
}

fn quota(max_concurrent: u32) -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota.max_concurrent = max_concurrent;
    policy
}

async fn guard_outputs_name_no_sandbox(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");

    // What the guard returns for a node to keep: the outcome and the collected result.
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    let events = env.journal.events();
    let id = ran.handle.sandbox_id().as_str().to_owned();
    let domain = events
        .iter()
        .find_map(|event| event.isolation_domain.clone())
        .map(|domain| domain.as_str().to_owned());
    let keys: Vec<String> = events.iter().map(|event| event.key.clone()).collect();
    let kept = format!(
        "{} {}",
        serde_json::to_string(&ran.outcome).unwrap_or_default(),
        serde_json::to_string(&ran.collected).unwrap_or_default()
    );
    ensure!(
        !kept.contains(&id),
        "what a node would keep names the sandbox"
    );
    ensure!(
        domain
            .as_deref()
            .is_none_or(|domain| !kept.contains(domain)),
        "what a node would keep names the isolation domain"
    );
    ensure!(
        keys.iter().all(|key| !kept.contains(key.as_str())),
        "what a node would keep holds an idempotency key"
    );
    // The handle, by contrast, does carry the sandbox: a node must not keep it.
    ensure!(
        serde_json::to_string(&ran.handle)
            .unwrap_or_default()
            .contains(&id),
        "the handle does not name its sandbox"
    );
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;

    // The node as a whole, run again on the same attempt after a restart of the host, gives
    // the same digests: the sandbox is rebuilt by replaying the node, not restored.
    let ctx = env.ctx("tenant-a", "node-2");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let state = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &Env::never(),
        Between::default(),
    )
    .await
    .must("the node")?;
    // The host restarted: the same node on a new guard, with the same keys.
    let again = env.restarted();
    let rerun = node_on(
        &env,
        &again,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &Env::never(),
        Between::default(),
    )
    .await
    .must("the node again")?;
    ensure!(rerun == state, "running the node again gave other digests");
    Ok(())
}

async fn no_sandbox_held_while_suspended(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(quota(1)).await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let never = Env::never();

    // No approval yet: the node suspends.
    let suspended = node(&env, &ctx, &exec, (None, None), &never, Between::default()).await;
    expect_code(&suspended, "approval_required", "a node with no approval")?;
    ensure!(env.tap.total() == 0, "a suspended node reached the backend");
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(
            live == 0,
            "{live} resources are held while the node is suspended"
        );
    }

    // The approval comes, and the node resumes with the same attempt.
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &never,
        Between::default(),
    )
    .await
    .must("the resumed node")?;
    ensure!(
        env.tap.count(SandboxStep::Create) == 1,
        "the resume created more than one sandbox"
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the resume ran more than one exec"
    );
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 1,
        "the sandbox of the node was not destroyed"
    );
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(live == 0, "{live} resources are held after the node");
    }
    if let Some(runs) = env.hooks.executions().await {
        ensure!(runs == 1, "the program ran {runs} times");
    }
    // The ceiling was one: the slot is free again.
    let next = env.ctx("tenant-a", "node-2");
    let claim = env
        .approve(&next, &env.create_req(), &exec)
        .must("approval request")?;
    node(
        &env,
        &next,
        &exec,
        (Some(&claim), Some(&claim)),
        &never,
        Between::default(),
    )
    .await
    .must("a node after the first")?;
    Ok(())
}

async fn cancellation_at_boundaries(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let flag = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&flag);
    let token = CancelToken::new(Arc::new(move || seen.load(Ordering::SeqCst)));
    let raise = |flag: &Arc<AtomicBool>| -> Step {
        let flag = Arc::clone(flag);
        Some(Arc::new(move || flag.store(true, Ordering::SeqCst)))
    };

    // Before create: nothing is created.
    flag.store(true, Ordering::SeqCst);
    let ctx = env.ctx("tenant-a", "before-create");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let result = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &token,
        Between::default(),
    )
    .await;
    expect_code(&result, "cancelled", "a node cancelled before create")?;
    ensure!(
        env.tap.total() == 0,
        "a node cancelled before create reached the backend"
    );

    // Between create and exec: the exec does not run, the sandbox is destroyed.
    flag.store(false, Ordering::SeqCst);
    let ctx = env.ctx("tenant-a", "before-exec");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let between = Between {
        before_exec: raise(&flag),
        ..Between::default()
    };
    let result = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &token,
        between,
    )
    .await;
    expect_code(&result, "cancelled", "a node cancelled before exec")?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "a cancelled exec ran"
    );
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 1,
        "the sandbox of a cancelled node was not destroyed"
    );

    // Between exec and collect: the collect does not run, the sandbox is destroyed.
    flag.store(false, Ordering::SeqCst);
    let ctx = env.ctx("tenant-a", "before-collect");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let between = Between {
        before_collect: raise(&flag),
        ..Between::default()
    };
    let result = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &token,
        between,
    )
    .await;
    expect_code(&result, "cancelled", "a node cancelled before collect")?;
    ensure!(
        env.tap.count(SandboxStep::Collect) == 0,
        "a cancelled collect reached the backend"
    );
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 2,
        "the sandbox of a cancelled node was not destroyed"
    );
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(
            live == 0,
            "{live} resources are held after a cancelled node"
        );
    }
    Ok(())
}

async fn sandbox_destroyed_on_every_exit(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let never = Env::never();

    // Success.
    let ctx = env.ctx("tenant-a", "success");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &never,
        Between::default(),
    )
    .await
    .must("the node")?;
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 1,
        "a node that succeeded did not destroy its sandbox"
    );

    // Failure: the backend fails the exec.
    let ctx = env.ctx("tenant-a", "failure");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    env.tap
        .fail_next(SandboxStep::Exec, SandboxError::Backend("x".to_owned()));
    let failed = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), Some(&claim)),
        &never,
        Between::default(),
    )
    .await;
    expect_code(&failed, "backend_error", "a node whose exec fails")?;
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 2,
        "a node that failed did not destroy its sandbox"
    );

    // Refusal: the exec is not approved.
    let ctx = env.ctx("tenant-a", "refusal");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let refused = node(
        &env,
        &ctx,
        &exec,
        (Some(&claim), None),
        &never,
        Between::default(),
    )
    .await;
    expect_code(
        &refused,
        "approval_required",
        "a node whose exec has no approval",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Destroy) == 3,
        "a node that was refused did not destroy its sandbox"
    );
    ensure!(
        env.kinds()
            .iter()
            .filter(|kind| **kind == SandboxEventKind::SandboxDestroyed)
            .count()
            == 3,
        "the destructions are not all journaled"
    );
    if let Some(live) = env.hooks.live_resources().await {
        ensure!(live == 0, "{live} resources are held after the nodes");
    }
    Ok(())
}
