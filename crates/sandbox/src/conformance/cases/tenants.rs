//! Multi-tenant: two tenants never share a sandbox or an isolation domain, a handle or a key
//! of another tenant is not found, and admission is per tenant, answered at once, so that a
//! tenant at its ceiling never slows another one down.

use super::{case, CaseDef};
use crate::conformance::support::{
    ensure, expect_code, fail, poll_once, shape, skip, BoxFut, Env, Lab, Must, Verdict,
};
use crate::error::{QuotaKind, SandboxError};
use crate::ids::{IsolationDomain, SandboxHandle, SandboxId, SandboxStep};
use crate::journal::SandboxEventKind;
use crate::policy::SandboxPolicy;
use std::collections::{HashMap, HashSet};

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Tenants,
            "tenants.no_shared_domain",
            RealBackend,
            "two tenants in parallel never get the same isolation domain, even after destroy",
            no_shared_domain
        ),
        case!(
            Tenants,
            "tenants.sandbox_ids_never_reused",
            RealBackend,
            "a sandbox id is unique and never reused, even after destroy",
            sandbox_ids_never_reused
        ),
        case!(
            Tenants,
            "tenants.shared_domain_detected",
            Guard,
            "a domain declared for two tenants is an isolation violation and the sandbox is torn down",
            shared_domain_detected
        ),
        case!(
            Tenants,
            "tenants.quota_immediate_and_fair",
            Guard,
            "a tenant at its ceiling is refused at once, and another tenant is still served",
            quota_immediate_and_fair
        ),
        case!(
            Tenants,
            "tenants.registry_share_per_tenant",
            Guard,
            "a tenant cannot fill the registry for the others",
            registry_share_per_tenant
        ),
        case!(
            Tenants,
            "tenants.not_found_across_tenants",
            Guard,
            "a handle of another tenant, or of another context, is not found",
            not_found_across_tenants
        ),
        case!(
            Tenants,
            "tenants.backend_not_found",
            RealBackend,
            "the backend answers not found for a handle or a key of another tenant, and touches nothing",
            backend_not_found
        ),
        case!(
            Tenants,
            "tenants.stores_apart",
            RealBackend,
            "the same node of two tenants never meets: each runs its own program in its own sandbox",
            stores_apart
        ),
    ]
}

async fn no_shared_domain(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let mut owner: HashMap<IsolationDomain, &str> = HashMap::new();
    let mut note = |domain: IsolationDomain, tenant: &'static str| -> Verdict {
        match owner.get(&domain) {
            Some(first) if *first != tenant => {
                fail!("two tenants were given the same isolation domain");
            }
            _ => {
                owner.insert(domain, tenant);
            }
        }
        Ok(())
    };
    // Two tenants in parallel: their sandboxes interleave.
    let mut live = Vec::new();
    for n in 0..3 {
        for tenant in ["tenant-a", "tenant-b"] {
            let ctx = env.ctx(tenant, &format!("node-{n}"));
            let (handle, raw) = env.b_create(&ctx, env.create_req()).await.must("create")?;
            note(raw.isolation_domain, tenant)?;
            live.push((ctx, handle));
        }
    }
    // One tenant's domains are released, and the other asks for new ones.
    for (ctx, handle) in live
        .iter()
        .filter(|(ctx, _)| ctx.tenant_id.as_str() == "tenant-a")
    {
        env.b_destroy(ctx, handle).await.must("destroy")?;
    }
    for n in 3..6 {
        let ctx = env.ctx("tenant-b", &format!("node-{n}"));
        let (_, raw) = env.b_create(&ctx, env.create_req()).await.must("create")?;
        note(raw.isolation_domain, "tenant-b")?;
    }
    for n in 6..9 {
        let ctx = env.ctx("tenant-a", &format!("node-{n}"));
        let (_, raw) = env.b_create(&ctx, env.create_req()).await.must("create")?;
        note(raw.isolation_domain, "tenant-a")?;
    }
    Ok(())
}

async fn sandbox_ids_never_reused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let mut seen: HashSet<SandboxId> = HashSet::new();
    for n in 0..5 {
        let ctx = env.ctx("tenant-a", &format!("node-{n}"));
        let (handle, raw) = env.b_create(&ctx, env.create_req()).await.must("create")?;
        ensure!(
            seen.insert(raw.sandbox_id),
            "a sandbox id was handed out twice"
        );
        // Destroyed, then created again: the id is not recycled.
        if n % 2 == 0 {
            env.b_destroy(&ctx, &handle).await.must("destroy")?;
        }
    }
    // A replayed create returns the same sandbox, not a new one.
    let ctx = env.ctx("tenant-a", "node-0");
    let (_, again) = env
        .b_create(&ctx, env.create_req())
        .await
        .must("replayed create")?;
    ensure!(
        seen.contains(&again.sandbox_id),
        "a replayed create returned a new sandbox"
    );
    Ok(())
}

async fn shared_domain_detected(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let a = env.ctx("tenant-a", "node-1");
    env.ready(&a, env.create_req(), &exec).await?;
    let taken = env
        .journal
        .events()
        .into_iter()
        .find(|event| event.kind == SandboxEventKind::SandboxCreated)
        .and_then(|event| event.isolation_domain);
    let Some(taken) = taken else {
        ensure!(
            false,
            "the created event does not name the isolation domain"
        );
        return Ok(());
    };
    // The backend declares tenant a's domain for tenant b.
    env.tap.rewrite_create(Box::new(move |result| {
        result.map(|mut raw| {
            raw.isolation_domain = taken.clone();
            raw
        })
    }));
    let b = env.ctx("tenant-b", "node-1");
    let claim = env
        .approve(&b, &env.create_req(), &exec)
        .must("approval request")?;
    let refused = env
        .guard
        .create(
            &b,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(
        &refused,
        "isolation_violation",
        "a domain declared for two tenants",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Destroy) >= 1,
        "the sandbox with the shared domain was not torn down"
    );
    ensure!(
        env.kinds().contains(&SandboxEventKind::SandboxDenied),
        "the violation is not in the journal"
    );
    Ok(())
}

async fn quota_immediate_and_fair(lab: &Lab<'_>) -> Verdict {
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota.max_concurrent = 2;
    let env = lab.env_with(policy).await;
    let exec = Env::success(lab)?;
    for n in 0..2 {
        env.ready(
            &env.ctx("tenant-a", &format!("node-{n}")),
            env.create_req(),
            &exec,
        )
        .await?;
    }
    let third = env.ctx("tenant-a", "node-2");
    let claim = env
        .approve(&third, &env.create_req(), &exec)
        .must("approval request")?;
    let never = Env::never();
    let mut call: BoxFut<'_, _> = Box::pin(env.guard.create(
        &third,
        env.create_req(),
        &exec,
        Some(&claim),
        env.run_started(),
        &never,
    ));
    match poll_once(&mut call) {
        Some(Err(SandboxError::QuotaExceeded {
            kind: QuotaKind::TenantConcurrency,
        })) => {}
        Some(other) => {
            let got = shape(&other);
            ensure!(
                false,
                "a tenant over its ceiling: expected quota_exceeded.tenant_concurrency, got {got}"
            );
        }
        None => ensure!(
            false,
            "a tenant over its ceiling was made to wait instead of being refused"
        ),
    }
    // Another tenant is served as usual.
    env.ready(&env.ctx("tenant-b", "node-0"), env.create_req(), &exec)
        .await?;

    // The same for the rate of creations.
    let mut policy = SandboxPolicy::default();
    policy.tenant_quota.max_creations = 2;
    let env = lab.env_with(policy).await;
    for n in 0..2 {
        let ctx = env.ctx("tenant-a", &format!("node-{n}"));
        let (handle, _) = env.ready(&ctx, env.create_req(), &exec).await?;
        env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    }
    let third = env.ctx("tenant-a", "node-2");
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
            &never,
        )
        .await;
    ensure!(
        matches!(
            &refused,
            Err(SandboxError::QuotaExceeded {
                kind: QuotaKind::CreationRate
            })
        ),
        "a tenant over its creation rate: expected quota_exceeded.creation_rate, got {}",
        shape(&refused)
    );
    env.ready(&env.ctx("tenant-b", "node-0"), env.create_req(), &exec)
        .await?;
    Ok(())
}

async fn registry_share_per_tenant(lab: &Lab<'_>) -> Verdict {
    let mut policy = SandboxPolicy::default();
    policy.max_records_per_tenant = 1;
    let env = lab.env_with(policy).await;
    let exec = Env::success(lab)?;
    env.ready(&env.ctx("tenant-a", "node-0"), env.create_req(), &exec)
        .await?;
    let second = env.ctx("tenant-a", "node-1");
    let claim = env
        .approve(&second, &env.create_req(), &exec)
        .must("approval request")?;
    let refused = env
        .guard
        .create(
            &second,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    ensure!(
        matches!(&refused, Err(SandboxError::QuotaExceeded { kind: QuotaKind::TenantRegistryFull })),
        "a tenant over its share of the registry: expected quota_exceeded.tenant_registry_full, got {}",
        shape(&refused)
    );
    env.ready(&env.ctx("tenant-b", "node-0"), env.create_req(), &exec)
        .await?;
    Ok(())
}

async fn not_found_across_tenants(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let a = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&a, env.create_req(), &exec).await?;
    env.run(&a, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    let calls = env.tap.total();
    let b = env.ctx("tenant-b", "node-1");
    let other_node = env.ctx("tenant-a", "node-2");
    for (ctx, who) in [(&b, "another tenant"), (&other_node, "another context")] {
        let result = env.run(ctx, &handle, exec.clone(), &claim).await;
        expect_code(&result, "not_found", &format!("exec by {who}"))?;
        let result = env.gather(ctx, &handle).await;
        expect_code(&result, "not_found", &format!("collect by {who}"))?;
        let result = env.guard.destroy(ctx, &handle).await;
        expect_code(&result, "not_found", &format!("destroy by {who}"))?;
    }
    ensure!(
        env.tap.total() == calls,
        "a handle of another tenant reached the backend"
    );
    // Nothing happened to the sandbox.
    env.gather(&a, &handle).await.must("collect by the owner")?;
    Ok(())
}

async fn backend_not_found(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let a = env.ctx("tenant-a", "node-1");
    let b = env.ctx("tenant-b", "node-1");
    let (ha, _) = env.b_create(&a, env.create_req()).await.must("create")?;
    env.b_exec(&a, &ha, exec.clone()).await.must("exec")?;

    // Tenant b holding tenant a's sandbox id.
    let forged = SandboxHandle::new(ha.sandbox_id().clone(), b.tenant_id.clone());
    expect_code(
        &env.b_exec(&b, &forged, exec.clone()).await,
        "not_found",
        "exec with a forged handle",
    )?;
    expect_code(
        &env.b_collect(&b, &forged).await,
        "not_found",
        "collect with a forged handle",
    )?;
    expect_code(
        &env.b_destroy(&b, &forged).await,
        "not_found",
        "destroy with a forged handle",
    )?;
    // Tenant a's handle under tenant b's keys.
    expect_code(
        &env.b_exec(&b, &ha, exec).await,
        "not_found",
        "exec with another tenant's keys",
    )?;
    // Nothing happened to tenant a's sandbox.
    env.b_collect(&a, &ha).await.must("collect by the owner")?;
    Ok(())
}

async fn stores_apart(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let Some(before) = env.hooks.executions().await else {
        skip!("the target cannot count how many times a program ran");
    };
    let a = env.ctx("tenant-a", "node-1");
    let b = env.ctx("tenant-b", "node-1");
    let (ha, ra) = env.b_create(&a, env.create_req()).await.must("create")?;
    env.b_exec(&a, &ha, exec.clone()).await.must("exec")?;
    let (hb, rb) = env.b_create(&b, env.create_req()).await.must("create")?;
    ensure!(
        ra.sandbox_id != rb.sandbox_id,
        "two tenants were given the same sandbox"
    );
    env.b_exec(&b, &hb, exec).await.must("exec")?;
    let runs = env.hooks.executions().await.unwrap_or(before) - before;
    ensure!(
        runs == 2,
        "the same node of two tenants ran {runs} programs, expected 2"
    );
    Ok(())
}
