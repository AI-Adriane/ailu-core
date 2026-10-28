//! Keys and cancellation: the keys of two different calls never collide, and a cancellation is
//! observed before the backend is asked.

use super::{case, CaseDef};
use crate::cancel::CancelToken;
use crate::conformance::support::{ensure, expect_code, Env, Lab, Must, Verdict};
use crate::ids::{IdempotencyKey, SandboxContext, SandboxStep, TenantId};
use ailu_graph_core::{NodeId, RunId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Keys,
            "keys.length_prefixed",
            Guard,
            "keys and call keys whose naive concatenation would collide stay distinct",
            length_prefixed
        ),
        case!(
            Keys,
            "keys.cancellation_before_every_call",
            Guard,
            "a cancelled token stops create, exec and collect before the backend is asked, and not destroy",
            cancellation_before_every_call
        ),
    ]
}

fn context(tenant: &str, run: &str, node: &str, attempt: u32) -> SandboxContext {
    SandboxContext {
        tenant_id: TenantId::new(tenant),
        run_id: RunId::from(run),
        node_id: NodeId::from(node),
        attempt,
    }
}

async fn length_prefixed(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    // The same characters, split differently between the segments.
    let a = context("a|b", "c", "node", 1);
    let b = context("a", "b|c", "node", 1);
    let c = context("a", "b", "c|node", 1);
    for (x, y) in [(&a, &b), (&b, &c), (&a, &c)] {
        for step in [SandboxStep::Create, SandboxStep::Exec] {
            ensure!(
                IdempotencyKey::new(x, step).as_str() != IdempotencyKey::new(y, step).as_str(),
                "two different contexts gave the same idempotency key"
            );
        }
        let one = env
            .guard
            .request_approval(x, &env.create_req(), &exec)
            .must("approval request")?;
        let other = env
            .guard
            .request_approval(y, &env.create_req(), &exec)
            .must("approval request")?;
        ensure!(
            one.call_key != other.call_key,
            "two different contexts gave the same call key"
        );
    }
    // The step and the attempt are part of the key.
    let keys = [
        IdempotencyKey::new(&a, SandboxStep::Create),
        IdempotencyKey::new(&a, SandboxStep::Exec),
        IdempotencyKey::new(&a, SandboxStep::Collect),
        IdempotencyKey::new(&a, SandboxStep::Destroy),
        IdempotencyKey::new(&context("a|b", "c", "node", 2), SandboxStep::Create),
    ];
    let distinct: std::collections::HashSet<_> = keys.iter().map(IdempotencyKey::as_str).collect();
    ensure!(
        distinct.len() == keys.len(),
        "a step or an attempt does not change the key"
    );
    ensure!(
        keys.iter().all(|key| key.as_str().len() == 64),
        "a key is not a sha256 hex digest"
    );
    Ok(())
}

async fn cancellation_before_every_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let flag = Arc::new(AtomicBool::new(true));
    let seen = Arc::clone(&flag);
    let cancelled = CancelToken::new(Arc::new(move || seen.load(Ordering::SeqCst)));
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;

    let created = env
        .guard
        .create(
            &ctx,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &cancelled,
        )
        .await;
    expect_code(&created, "cancelled", "a create with a cancelled token")?;
    ensure!(
        env.tap.total() == 0,
        "a cancelled create reached the backend"
    );
    flag.store(false, Ordering::SeqCst);
    let handle = env
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
        .must("the create once the token is clear")?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;

    flag.store(true, Ordering::SeqCst);
    let calls = env.tap.total();
    let collected = env
        .guard
        .collect(
            &ctx,
            &handle,
            env.collect_req(),
            env.run_started(),
            &cancelled,
        )
        .await;
    expect_code(&collected, "cancelled", "a collect with a cancelled token")?;
    ensure!(
        env.tap.total() == calls,
        "a cancelled collect reached the backend"
    );
    flag.store(false, Ordering::SeqCst);
    env.gather(&ctx, &handle)
        .await
        .must("the collect once the token is clear")?;
    // Destroy takes no token: it is never cancelled.
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    Ok(())
}
