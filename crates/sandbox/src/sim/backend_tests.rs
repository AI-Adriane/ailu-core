//! The obligations of a backend, checked on the simulator directly (no guard in between), so
//! that what the guard would hide is seen.

use super::test_util::*;
use super::*;
use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::collected::RawCollected;
use crate::error::{SandboxError, SandboxState};
use crate::ids::{IdempotencyKey, SandboxHandle, SandboxStep, TenantId};
use crate::outcome::ExecOutcome;
use crate::request::{CollectRequest, CreateRequest, ExecRequest};
use crate::{RawCreated, SandboxContext};
use std::collections::HashSet;
use std::sync::Arc;

type ExecResult = Result<ExecOutcome, SandboxError>;

fn never() -> CancelToken {
    CancelToken::never()
}

fn key(ctx: &SandboxContext, step: SandboxStep) -> IdempotencyKey {
    IdempotencyKey::new(ctx, step)
}

async fn create(
    sim: &SimBackend,
    ctx: &SandboxContext,
    req: CreateRequest,
) -> (SandboxHandle, RawCreated) {
    let raw = sim
        .create(ctx, &key(ctx, SandboxStep::Create), req, &never())
        .await
        .unwrap();
    (SandboxHandle::from_created(&raw, &ctx.tenant_id), raw)
}

async fn exec(
    sim: &SimBackend,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
    req: ExecRequest,
) -> ExecResult {
    sim.exec(handle, &key(ctx, SandboxStep::Exec), req, &never())
        .await
}

async fn collect(
    sim: &SimBackend,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
    req: CollectRequest,
) -> Result<RawCollected, SandboxError> {
    sim.collect(handle, &key(ctx, SandboxStep::Collect), req, &never())
        .await
}

async fn destroy(
    sim: &SimBackend,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
) -> Result<(), SandboxError> {
    sim.destroy(handle, &key(ctx, SandboxStep::Destroy)).await
}

// ---- deduplication on the key ---------------------------------------------------------

#[tokio::test]
async fn twenty_concurrent_calls_with_one_key_run_the_program_once() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let (sim, ctx, handle) = (sim.clone(), ctx.clone(), handle.clone());
        tasks.push(tokio::spawn(async move {
            exec(&sim, &ctx, &handle, exec_req()).await
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap().unwrap());
    }
    assert!(
        results.windows(2).all(|w| w[0] == w[1]),
        "one result for all"
    );
    assert_eq!(sim.program_calls(), 1);
}

#[test]
fn a_call_that_finds_the_execution_under_way_is_told_to_wait_and_the_program_runs_once() {
    use std::sync::Barrier;
    let (sim, _) = bare();
    let gate = Arc::new(Barrier::new(2));
    let inside = Arc::clone(&gate);
    sim.register("hold", move |_| {
        inside.wait();
        Ok(SimOutput::success())
    });
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    };
    let ctx = ctx();
    let (handle, _) = runtime().block_on(create(&sim, &ctx, create_req()));
    let request = exec_of("hold", &[]);
    let mut threads = Vec::new();
    for _ in 0..2 {
        let (sim, ctx, handle, request) =
            (sim.clone(), ctx.clone(), handle.clone(), request.clone());
        let gate = Arc::clone(&gate);
        threads.push(std::thread::spawn(move || {
            let result = runtime().block_on(exec(&sim, &ctx, &handle, request));
            if matches!(result, Err(SandboxError::Busy { .. })) {
                // The caller that was told to wait lets the running program finish.
                gate.wait();
            }
            result
        }));
    }
    let results: Vec<ExecResult> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(SandboxError::Busy { .. })))
            .count(),
        1
    );
    assert_eq!(sim.program_calls(), 1);
    // Once it finished, the same call has its answer.
    let again = runtime().block_on(exec(&sim, &ctx, &handle, request));
    assert!(again.is_ok());
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn the_same_key_with_another_request_is_a_conflict_in_every_state() {
    let (sim, _) = bare();
    let ctx = ctx();
    let mut other_create = create_req();
    other_create.ttl_ms += 1;
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let conflict = Err(SandboxError::IdempotencyConflict);
    let other_collect = CollectRequest {
        patterns: vec!["*".to_owned()],
        ..collect_req()
    };

    // Created: only `create` has a record yet.
    assert_eq!(
        sim.create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            other_create.clone(),
            &never()
        )
        .await
        .map(|_| ()),
        conflict
    );
    // Executed.
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_of("echo", &["different"]))
            .await
            .map(|_| ()),
        conflict
    );
    // Collected.
    collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    assert_eq!(
        collect(&sim, &ctx, &handle, other_collect.clone())
            .await
            .map(|_| ()),
        conflict
    );
    // Destroyed: the records outlive the sandbox.
    destroy(&sim, &ctx, &handle).await.unwrap();
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_of("echo", &["different"]))
            .await
            .map(|_| ()),
        conflict
    );
    assert_eq!(
        collect(&sim, &ctx, &handle, other_collect)
            .await
            .map(|_| ()),
        conflict
    );
    assert_eq!(
        sim.create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            other_create,
            &never()
        )
        .await
        .map(|_| ()),
        conflict
    );
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn answers_survive_destroy_for_the_retention_and_no_longer() {
    let (sim, clock) = bare_with(SimConfig {
        retention_ms: 10_000,
        ..SimConfig::default()
    });
    let ctx = ctx();
    let (handle, raw) = create(&sim, &ctx, create_req()).await;
    let first = exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    let collected = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    destroy(&sim, &ctx, &handle).await.unwrap();
    destroy(&sim, &ctx, &handle).await.unwrap();
    assert_eq!(sim.live_sandboxes(), 0);

    // Inside the retention the stored answers come back, with no sandbox and no new run.
    clock.advance(9_999);
    assert_eq!(exec(&sim, &ctx, &handle, exec_req()).await.unwrap(), first);
    let again = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    assert_eq!(again.stdout, collected.stdout);
    assert_eq!(again.files.len(), collected.files.len());
    let created = sim
        .create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            create_req(),
            &never(),
        )
        .await
        .unwrap();
    assert_eq!(created, raw);
    assert_eq!(sim.program_calls(), 1);

    // Past it, the key is forgotten and the sandbox is gone: there is nothing to run on.
    clock.advance(1);
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
        Err(SandboxError::NotFound)
    );
    assert_eq!(sim.program_calls(), 1);
}

// ---- marker ---------------------------------------------------------------------------

#[tokio::test]
async fn the_marker_is_written_before_the_program_runs() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let probe = sim.clone();
    let tenant = ctx.tenant_id.clone();
    sim.register("probe", move |_| {
        // Inside the program: the create record and the exec marker are both in the store.
        assert_eq!(probe.stored_records(&tenant), 2);
        Ok(SimOutput::success())
    });
    exec(&sim, &ctx, &handle, exec_of("probe", &[]))
        .await
        .unwrap();
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn a_marker_without_a_result_and_without_a_resource_is_final() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    sim.inject(SandboxStep::Exec, SimFault::CrashAfterMarker)
        .unwrap();
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
        Err(SandboxError::Unavailable { retriable: true })
    );
    for _ in 0..3 {
        assert_eq!(
            exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
            Err(SandboxError::Unavailable { retriable: false })
        );
    }
    assert_eq!(sim.program_calls(), 1, "never rerun");
    // Nothing can be collected from an execution whose outcome is unknown.
    assert_eq!(
        collect(&sim, &ctx, &handle, collect_req())
            .await
            .map(|_| ()),
        Err(SandboxError::InvalidState {
            from: SandboxState::Executing
        })
    );
}

#[tokio::test]
async fn a_scripted_refusal_leaves_no_marker_and_a_replay_does_not_consume_a_fault() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    sim.inject(SandboxStep::Exec, SimFault::unavailable(true))
        .unwrap();
    assert!(exec(&sim, &ctx, &handle, exec_req()).await.is_err());
    // No marker: the retry runs the program.
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    assert_eq!(sim.program_calls(), 1);
    // A fault queued now waits for a call that is not a replay.
    sim.inject(SandboxStep::Exec, SimFault::busy(None)).unwrap();
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    assert_eq!(sim.program_calls(), 1);
    sim.clear_faults();
}

#[tokio::test]
async fn a_program_that_panics_leaves_a_marker_and_is_not_run_again() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let first = exec(&sim, &ctx, &handle, exec_of("panic", &[])).await;
    assert_eq!(first.map(|_| ()).unwrap_err().code(), "backend_error");
    let second = exec(&sim, &ctx, &handle, exec_of("panic", &[])).await;
    assert_eq!(
        second.map(|_| ()),
        Err(SandboxError::Unavailable { retriable: false })
    );
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn an_unknown_program_is_a_typed_error_with_no_effect() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let error = exec(&sim, &ctx, &handle, exec_of("no-such-program", &[]))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "backend_error");
    assert_eq!(sim.program_calls(), 0);
    assert_eq!(
        sim.stored_records(&ctx.tenant_id),
        1,
        "no marker was written"
    );
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
}

// ---- tenants and handles --------------------------------------------------------------

#[tokio::test]
async fn another_tenants_handle_or_key_is_not_found() {
    let (sim, _) = bare();
    let a = ctx_for("tenant-a", "node-1");
    let b = ctx_for("tenant-b", "node-1");
    let (ha, _) = create(&sim, &a, create_req()).await;
    exec(&sim, &a, &ha, exec_req()).await.unwrap();

    // Tenant B holding tenant A's sandbox id.
    let forged = SandboxHandle::new(ha.sandbox_id().clone(), b.tenant_id.clone());
    assert_eq!(
        exec(&sim, &b, &forged, exec_req()).await.map(|_| ()),
        Err(SandboxError::NotFound)
    );
    assert_eq!(
        collect(&sim, &b, &forged, collect_req()).await.map(|_| ()),
        Err(SandboxError::NotFound)
    );
    assert_eq!(
        destroy(&sim, &b, &forged).await,
        Err(SandboxError::NotFound)
    );
    // Tenant A's handle under tenant B's keys, and the reverse.
    assert_eq!(
        exec(&sim, &b, &ha, exec_req()).await.map(|_| ()),
        Err(SandboxError::NotFound)
    );
    assert_eq!(
        sim.exec(&forged, &key(&a, SandboxStep::Exec), exec_req(), &never())
            .await
            .map(|_| ()),
        Err(SandboxError::NotFound)
    );
    // Nothing happened to tenant A's sandbox.
    assert_eq!(sim.live_sandboxes(), 1);
    collect(&sim, &a, &ha, collect_req()).await.unwrap();

    // The durable stores are separate: the same node of two tenants never meets.
    let (hb, _) = create(&sim, &b, create_req()).await;
    assert_ne!(ha.sandbox_id(), hb.sandbox_id());
    exec(&sim, &b, &hb, exec_req()).await.unwrap();
    assert_eq!(
        sim.program_calls(),
        2,
        "tenant B's call is not tenant A's replay"
    );
    assert_eq!(sim.stored_records(&a.tenant_id), 3);
    assert_eq!(sim.stored_records(&b.tenant_id), 2);
}

#[tokio::test]
async fn each_sandbox_has_its_own_domain_and_no_domain_is_shared_between_tenants() {
    let (sim, _) = bare();
    let mut seen = HashSet::new();
    for tenant in ["tenant-a", "tenant-b"] {
        for n in 0..25 {
            let ctx = ctx_for(tenant, &format!("node-{n}"));
            let (handle, raw) = create(&sim, &ctx, create_req()).await;
            assert!(
                seen.insert(raw.isolation_domain.clone()),
                "unique per sandbox"
            );
            assert_eq!(
                sim.domain_owner(raw.isolation_domain.as_str()),
                Some(TenantId::new(tenant))
            );
            destroy(&sim, &ctx, &handle).await.unwrap();
        }
    }
    // A domain is not recycled after destroy, either.
    let (_, raw) = create(&sim, &ctx_for("tenant-a", "again"), create_req()).await;
    assert!(seen.insert(raw.isolation_domain));
}

#[tokio::test]
async fn the_shared_domain_fault_returns_a_domain_of_another_tenant() {
    let (sim, _) = bare();
    let a = ctx_for("tenant-a", "node-1");
    let b = ctx_for("tenant-b", "node-1");
    let (_, raw_a) = create(&sim, &a, create_req()).await;
    sim.inject(SandboxStep::Create, SimFault::SharedDomain)
        .unwrap();
    let (_, raw_b) = create(&sim, &b, create_req()).await;
    assert_eq!(raw_b.isolation_domain, raw_a.isolation_domain);
    // With nobody else yet, the fault still hands out a domain the next tenant will find taken.
    let (sim, _) = bare();
    sim.inject(SandboxStep::Create, SimFault::SharedDomain)
        .unwrap();
    sim.inject(SandboxStep::Create, SimFault::SharedDomain)
        .unwrap();
    let (_, first) = create(&sim, &a, create_req()).await;
    let (_, second) = create(&sim, &b, create_req()).await;
    assert_eq!(first.isolation_domain, second.isolation_domain);
}

// ---- lifecycle ------------------------------------------------------------------------

#[tokio::test]
async fn a_sandbox_runs_one_exec() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    // Another attempt of the node has another key, and the same sandbox.
    let mut second = ctx.clone();
    second.attempt = 2;
    let result = sim
        .exec(
            &handle,
            &key(&second, SandboxStep::Exec),
            exec_req(),
            &never(),
        )
        .await;
    assert_eq!(
        result.map(|_| ()),
        Err(SandboxError::InvalidState {
            from: SandboxState::Executed
        })
    );
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn collect_before_exec_is_an_invalid_state_and_destroy_of_nothing_succeeds() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    assert_eq!(
        collect(&sim, &ctx, &handle, collect_req())
            .await
            .map(|_| ()),
        Err(SandboxError::InvalidState {
            from: SandboxState::Created
        })
    );
    destroy(&sim, &ctx, &handle).await.unwrap();
    destroy(&sim, &ctx, &handle).await.unwrap();
    let stranger = SandboxHandle::new(
        crate::ids::SandboxId::new("never-existed"),
        ctx.tenant_id.clone(),
    );
    destroy(&sim, &ctx, &stranger).await.unwrap();
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
        Err(SandboxError::NotFound),
        "a destroyed sandbox runs nothing new"
    );
}

#[tokio::test]
async fn a_cancelled_token_stops_every_call_before_any_effect() {
    let (sim, _) = bare();
    let ctx = ctx();
    let cancelled = CancelToken::new(Arc::new(|| true));
    let created = sim
        .create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            create_req(),
            &cancelled,
        )
        .await;
    assert_eq!(created.map(|_| ()), Err(SandboxError::Cancelled));
    assert_eq!(
        (sim.live_sandboxes(), sim.stored_records(&ctx.tenant_id)),
        (0, 0)
    );

    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let records = sim.stored_records(&ctx.tenant_id);
    let result = sim
        .exec(
            &handle,
            &key(&ctx, SandboxStep::Exec),
            exec_req(),
            &cancelled,
        )
        .await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::Cancelled));
    assert_eq!(sim.program_calls(), 0);
    assert_eq!(sim.stored_records(&ctx.tenant_id), records, "no marker");

    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    let result = sim
        .collect(
            &handle,
            &key(&ctx, SandboxStep::Collect),
            collect_req(),
            &cancelled,
        )
        .await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::Cancelled));
    collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
}

#[tokio::test]
async fn the_ttl_makes_a_sandbox_unreachable_and_the_sweep_removes_it() {
    let (sim, clock) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    clock.advance(60_000);
    assert_eq!(sim.residue(), 1);
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
        Err(SandboxError::NotFound),
        "past its ttl, before any sweep"
    );
    assert_eq!(sim.live_sandboxes(), 1, "still there until swept");
    assert_eq!(
        sim.sweep(),
        SweepReport {
            sandboxes: 1,
            records: 0
        }
    );
    assert_eq!((sim.live_sandboxes(), sim.residue()), (0, 0));
}

#[tokio::test]
async fn the_number_of_records_per_tenant_is_bounded() {
    let (sim, clock) = bare_with(SimConfig {
        max_records_per_tenant: 3,
        retention_ms: 5_000,
        ..SimConfig::default()
    });
    let a = |n: u32| ctx_for("tenant-a", &format!("node-{n}"));
    for n in 0..3 {
        create(&sim, &a(n), create_req()).await;
    }
    let refused = sim
        .create(
            &a(9),
            &key(&a(9), SandboxStep::Create),
            create_req(),
            &never(),
        )
        .await;
    assert_eq!(
        refused.map(|_| ()),
        Err(SandboxError::Busy {
            retry_after_ms: None
        })
    );
    // Another tenant is not affected, and the bound lifts when records expire.
    create(&sim, &ctx_for("tenant-b", "node-0"), create_req()).await;
    clock.advance(5_000);
    create(&sim, &a(9), create_req()).await;
}

// ---- outputs --------------------------------------------------------------------------

#[tokio::test]
async fn collect_is_bounded_by_the_request_and_marks_what_it_cut() {
    let (sim, _) = bare();
    sim.register("many", |inv| {
        for n in 1..=5 {
            inv.fs()
                .write(&format!("out/{n}.txt"), vec![b'x'; 10])
                .unwrap();
        }
        inv.fs().write("elsewhere/skip.txt", "no").unwrap();
        Ok(SimOutput::success())
    });
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    exec(&sim, &ctx, &handle, exec_of("many", &[]))
        .await
        .unwrap();
    let request = CollectRequest {
        patterns: vec!["out/*.txt".to_owned()],
        max_files: 3,
        max_bytes: 1000,
    };
    let raw = collect(&sim, &ctx, &handle, request).await.unwrap();
    assert_eq!(raw.files.len(), 3);
    assert!(raw.declared_truncated);
    assert!(raw
        .files
        .iter()
        .all(|f| f.path.as_str().starts_with("out/")));

    let ctx2 = ctx_for("tenant-a", "node-2");
    let (handle, _) = create(&sim, &ctx2, create_req()).await;
    exec(&sim, &ctx2, &handle, exec_of("many", &[]))
        .await
        .unwrap();
    let request = CollectRequest {
        patterns: vec!["out/*.txt".to_owned()],
        max_files: 10,
        max_bytes: 25,
    };
    let raw = collect(&sim, &ctx2, &handle, request).await.unwrap();
    assert_eq!(raw.files.len(), 2, "the third file would pass 25 bytes");
    assert!(raw.declared_truncated);

    let ctx3 = ctx_for("tenant-a", "node-3");
    let (handle, _) = create(&sim, &ctx3, create_req()).await;
    exec(&sim, &ctx3, &handle, exec_of("many", &[]))
        .await
        .unwrap();
    let request = CollectRequest {
        patterns: vec!["out/*.txt".to_owned()],
        max_files: 10,
        max_bytes: 1000,
    };
    let raw = collect(&sim, &ctx3, &handle, request).await.unwrap();
    assert_eq!((raw.files.len(), raw.declared_truncated), (5, false));
}

#[tokio::test]
async fn what_the_backend_keeps_serializes_and_holds_no_secret() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    let raw = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    let json = serde_json::to_string(&raw).unwrap();
    let back: RawCollected = serde_json::from_str(&json).unwrap();
    assert_eq!(back.stdout, raw.stdout);
    assert_eq!(back.files.len(), raw.files.len());
    assert!(back.secrets.is_empty(), "values are never serialized");
}

#[tokio::test]
async fn output_is_cut_at_the_bound_even_when_the_limit_is_absurd() {
    let (sim, _) = bare();
    let ctx = ctx();
    let mut create_request = create_req();
    create_request.limits.output_bytes = u64::MAX;
    let (handle, _) = create(&sim, &ctx, create_request).await;
    let outcome = exec(&sim, &ctx, &handle, exec_of("flood", &[]))
        .await
        .unwrap();
    assert!(outcome.truncated);
    let raw = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    assert_eq!(
        raw.stdout.len() as u64,
        MAX_STREAM_BYTES,
        "memory stays bounded"
    );
}

#[tokio::test]
async fn a_disk_too_small_for_the_inputs_is_refused_at_create() {
    let (sim, _) = bare();
    let ctx = ctx();
    let reference = ailu_fs_backend::ArtifactRef {
        id: "run-1:big".into(),
        version: 1,
    };
    sim.provide_artifact(&ctx.tenant_id, reference.clone(), vec![0u8; 100]);
    let mut request = create_req();
    request.limits.ephemeral_disk_bytes = 10;
    request.inputs = vec![crate::request::InputRef {
        artifact: reference,
        dest: path("in/big.bin"),
    }];
    let result = sim
        .create(&ctx, &key(&ctx, SandboxStep::Create), request, &never())
        .await;
    assert_eq!(result.map(|_| ()).unwrap_err().code(), "backend_error");
    assert_eq!(sim.live_sandboxes(), 0);
}

#[tokio::test]
async fn a_key_of_another_operation_addresses_nothing() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let wrong = key(&ctx, SandboxStep::Collect);
    let result = sim.exec(&handle, &wrong, exec_req(), &never()).await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::NotFound));
    let result = sim
        .create(&ctx, &key(&ctx, SandboxStep::Exec), create_req(), &never())
        .await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::NotFound));
    assert_eq!(sim.program_calls(), 0);
}
