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

/// The attempt deadline the direct calls pass: within what the simulator keeps.
const DEADLINE_MS: u64 = NOW + 3_600_000;

fn key(ctx: &SandboxContext, step: SandboxStep) -> IdempotencyKey {
    IdempotencyKey::new(ctx, step)
}

async fn create(
    sim: &SimBackend,
    ctx: &SandboxContext,
    req: CreateRequest,
) -> (SandboxHandle, RawCreated) {
    let raw = sim
        .create(
            ctx,
            &key(ctx, SandboxStep::Create),
            req,
            DEADLINE_MS,
            &never(),
        )
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
    sim.exec(
        handle,
        &key(ctx, SandboxStep::Exec),
        req,
        DEADLINE_MS,
        &never(),
    )
    .await
}

async fn collect(
    sim: &SimBackend,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
    req: CollectRequest,
) -> Result<RawCollected, SandboxError> {
    sim.collect(
        handle,
        &key(ctx, SandboxStep::Collect),
        req,
        DEADLINE_MS,
        &never(),
    )
    .await
}

async fn destroy(
    sim: &SimBackend,
    ctx: &SandboxContext,
    handle: &SandboxHandle,
) -> Result<(), SandboxError> {
    sim.destroy(handle, &key(ctx, SandboxStep::Destroy)).await
}

// ---- the attempt deadline -------------------------------------------------------------

#[tokio::test]
async fn the_backend_reports_the_retention_it_promises() {
    let (sim, _) = bare();
    assert_eq!(sim.max_retention(), sim.retention_ms());
}

#[tokio::test]
async fn a_deadline_beyond_what_the_backend_keeps_is_refused_when_the_key_is_sealed() {
    let (sim, _) = bare();
    let ctx = ctx();
    let too_late = NOW + sim.max_retention() + 1;
    let key = key(&ctx, SandboxStep::Create);
    let refused = sim
        .create(&ctx, &key, create_req(), too_late, &never())
        .await;
    assert!(refused.is_err());
    // Nothing was created or remembered.
    assert_eq!(sim.live_sandboxes(), 0);
    assert_eq!(sim.stored_records(&ctx.tenant_id), 0);
    // The largest deadline it can keep is accepted.
    let ok = sim
        .create(
            &ctx,
            &key,
            create_req(),
            NOW + sim.max_retention(),
            &never(),
        )
        .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn a_repeat_at_the_sealed_deadline_itself_is_still_answered_from_the_record() {
    // The guard accepts a repeat at the deadline itself, so the backend must still hold the key
    // there: a deadline sealed at the limit of what the backend keeps, and the clock on it.
    let (sim, clock) = bare();
    let ctx = ctx();
    let deadline = NOW + sim.max_retention();
    let create_key = key(&ctx, SandboxStep::Create);
    let raw = sim
        .create(&ctx, &create_key, create_req(), deadline, &never())
        .await
        .unwrap();
    let handle = SandboxHandle::from_created(&raw, &ctx.tenant_id);
    let exec_key = key(&ctx, SandboxStep::Exec);
    let first = sim
        .exec(&handle, &exec_key, exec_req(), deadline, &never())
        .await
        .unwrap();
    let collect_key = key(&ctx, SandboxStep::Collect);
    let collected = sim
        .collect(&handle, &collect_key, collect_req(), deadline, &never())
        .await
        .unwrap();

    clock.advance(sim.max_retention());
    let again = sim
        .create(&ctx, &create_key, create_req(), deadline, &never())
        .await
        .unwrap();
    assert_eq!(again, raw);
    assert_eq!(
        sim.exec(&handle, &exec_key, exec_req(), deadline, &never())
            .await
            .unwrap(),
        first
    );
    let collected_again = sim
        .collect(&handle, &collect_key, collect_req(), deadline, &never())
        .await
        .unwrap();
    assert_eq!(collected_again.stdout, collected.stdout);
    assert_eq!(sim.live_sandboxes(), 1);
    assert_eq!(sim.program_calls(), 1);
}

#[tokio::test]
async fn a_repeated_key_may_not_ask_for_a_later_deadline_than_it_was_sealed_with() {
    let (sim, _) = bare();
    let ctx = ctx();
    let key = key(&ctx, SandboxStep::Create);
    let first = sim
        .create(&ctx, &key, create_req(), DEADLINE_MS, &never())
        .await
        .unwrap();
    // The same or an earlier deadline answers from the record.
    for deadline in [DEADLINE_MS, DEADLINE_MS - 1] {
        let again = sim
            .create(&ctx, &key, create_req(), deadline, &never())
            .await
            .unwrap();
        assert_eq!(again.sandbox_id, first.sandbox_id);
    }
    // A later one is refused, and the program-less record is untouched.
    let later = sim
        .create(&ctx, &key, create_req(), DEADLINE_MS + 1, &never())
        .await;
    assert!(later.is_err());
}

#[tokio::test]
async fn exec_and_collect_seal_and_repeat_with_the_same_rules() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let too_late = NOW + sim.max_retention() + 1;
    let exec_key = key(&ctx, SandboxStep::Exec);
    assert!(sim
        .exec(&handle, &exec_key, exec_req(), too_late, &never())
        .await
        .is_err());
    assert_eq!(sim.program_calls(), 0, "refused before anything ran");
    sim.exec(&handle, &exec_key, exec_req(), DEADLINE_MS, &never())
        .await
        .unwrap();
    assert!(sim
        .exec(&handle, &exec_key, exec_req(), DEADLINE_MS + 1, &never())
        .await
        .is_err());
    assert_eq!(sim.program_calls(), 1, "never run again");

    let collect_key = key(&ctx, SandboxStep::Collect);
    assert!(sim
        .collect(&handle, &collect_key, collect_req(), too_late, &never())
        .await
        .is_err());
    sim.collect(&handle, &collect_key, collect_req(), DEADLINE_MS, &never())
        .await
        .unwrap();
    assert!(sim
        .collect(
            &handle,
            &collect_key,
            collect_req(),
            DEADLINE_MS + 1,
            &never()
        )
        .await
        .is_err());
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
    use std::sync::mpsc;
    use std::time::Duration;
    let (sim, _) = bare();
    let (release, released) = mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    sim.register("hold", move |_| {
        // Waits for the caller that was told to wait; a regression times out instead of
        // hanging the test run.
        let waited = released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10));
        assert!(waited.is_ok(), "nobody was told to wait");
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
        let release = release.clone();
        threads.push(std::thread::spawn(move || {
            let result = runtime().block_on(exec(&sim, &ctx, &handle, request));
            if matches!(result, Err(SandboxError::Busy { .. })) {
                // The caller that was told to wait lets the running program finish.
                release.send(()).unwrap();
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

#[test]
fn twenty_threads_with_one_key_never_call_the_program_twice() {
    let (sim, _) = bare();
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    };
    let ctx = ctx();
    let (handle, _) = runtime().block_on(create(&sim, &ctx, create_req()));
    // A program slow enough for the threads to overlap.
    sim.register("slow", |_| {
        std::thread::sleep(std::time::Duration::from_millis(30));
        Ok(SimOutput::success().with_stdout("done"))
    });
    let request = exec_of("slow", &[]);
    let threads: Vec<_> = (0..20)
        .map(|_| {
            let (sim, ctx, handle, request) =
                (sim.clone(), ctx.clone(), handle.clone(), request.clone());
            std::thread::spawn(move || runtime().block_on(exec(&sim, &ctx, &handle, request)))
        })
        .collect();
    let results: Vec<ExecResult> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        sim.program_calls(),
        1,
        "one call, whatever the interleaving"
    );
    let done: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert!(!done.is_empty());
    assert!(done.windows(2).all(|w| w[0] == w[1]), "the same result");
    for result in &results {
        if let Err(error) = result {
            assert!(matches!(error, SandboxError::Busy { .. }), "{error:?}");
        }
    }
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
            DEADLINE_MS,
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
            DEADLINE_MS,
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
    let (sim, clock) = bare();
    let retention = sim.retention_ms();
    let ctx = ctx();
    let (handle, raw) = create(&sim, &ctx, create_req()).await;
    let first = exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    let collected = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    destroy(&sim, &ctx, &handle).await.unwrap();
    destroy(&sim, &ctx, &handle).await.unwrap();
    assert_eq!(sim.live_sandboxes(), 0);

    // Through the last instant of the retention the stored answers come back, with no sandbox and
    // no new run.
    clock.advance(retention);
    assert_eq!(exec(&sim, &ctx, &handle, exec_req()).await.unwrap(), first);
    let again = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    assert_eq!(again.stdout, collected.stdout);
    assert_eq!(again.files.len(), collected.files.len());
    let created = sim
        .create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            create_req(),
            DEADLINE_MS,
            &never(),
        )
        .await
        .unwrap();
    assert_eq!(created, raw);
    assert_eq!(sim.program_calls(), 1);

    // One millisecond later the key is forgotten and the sandbox is gone: there is nothing to run
    // on.
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
        sim.exec(
            &forged,
            &key(&a, SandboxStep::Exec),
            exec_req(),
            DEADLINE_MS,
            &never()
        )
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
            DEADLINE_MS,
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
            DEADLINE_MS,
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
            DEADLINE_MS,
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
            DEADLINE_MS,
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
            DEADLINE_MS,
            &never(),
        )
        .await;
    assert_eq!(
        refused.map(|_| ()),
        Err(SandboxError::QuotaExceeded {
            kind: crate::error::QuotaKind::TenantRegistryFull
        })
    );
    // Another tenant is not affected, and the bound lifts when records expire.
    create(&sim, &ctx_for("tenant-b", "node-0"), create_req()).await;
    clock.advance(sim.retention_ms() + 1);
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
        .create(
            &ctx,
            &key(&ctx, SandboxStep::Create),
            request,
            DEADLINE_MS,
            &never(),
        )
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
    let result = sim
        .exec(&handle, &wrong, exec_req(), DEADLINE_MS, &never())
        .await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::NotFound));
    let result = sim
        .create(
            &ctx,
            &key(&ctx, SandboxStep::Exec),
            create_req(),
            DEADLINE_MS,
            &never(),
        )
        .await;
    assert_eq!(result.map(|_| ()), Err(SandboxError::NotFound));
    assert_eq!(sim.program_calls(), 0);
}

#[tokio::test]
async fn a_second_collect_under_another_key_is_an_invalid_state() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    let first = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    // The same call again, with the same key, returns the recorded result.
    let again = collect(&sim, &ctx, &handle, collect_req()).await.unwrap();
    assert_eq!(again.stdout, first.stdout);
    // Another key (another attempt of the node) finds the sandbox already collected.
    let mut second = ctx.clone();
    second.attempt = 2;
    let result = sim
        .collect(
            &handle,
            &key(&second, SandboxStep::Collect),
            collect_req(),
            DEADLINE_MS,
            &never(),
        )
        .await;
    assert_eq!(
        result.map(|_| ()),
        Err(SandboxError::InvalidState {
            from: SandboxState::Collected
        })
    );
}

#[tokio::test]
async fn an_outgoing_link_is_refused_at_collect_and_an_inside_link_is_followed() {
    let (sim, _) = bare();
    sim.register("links", |inv| {
        inv.fs().write("out/real.txt", "real").unwrap();
        inv.fs().symlink("out/inside", "out/real.txt").unwrap();
        if inv.args().first().map(String::as_str) == Some("escape") {
            inv.fs().symlink("out/escape", "../../etc/passwd").unwrap();
        }
        if inv.args().first().map(String::as_str) == Some("absolute") {
            inv.fs().symlink("out/abs", "/etc/passwd").unwrap();
        }
        Ok(SimOutput::success())
    });
    for (node, arg, ok) in [
        ("n1", "none", true),
        ("n2", "escape", false),
        ("n3", "absolute", false),
    ] {
        let ctx = ctx_for("tenant-a", node);
        let (handle, _) = create(&sim, &ctx, create_req()).await;
        exec(&sim, &ctx, &handle, exec_of("links", &[arg]))
            .await
            .unwrap();
        let request = CollectRequest {
            patterns: vec!["out/*".to_owned()],
            max_files: 8,
            max_bytes: 100,
        };
        let result = collect(&sim, &ctx, &handle, request).await;
        if ok {
            let raw = result.unwrap();
            let paths: Vec<_> = raw.files.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(paths, vec!["out/inside", "out/real.txt"]);
            assert_eq!(raw.files[0].bytes, b"real");
        } else {
            assert_eq!(result.map(|_| ()).unwrap_err().code(), "invalid_path");
        }
    }
}

#[tokio::test]
async fn too_many_files_in_the_sandbox_exceed_the_disk_limit() {
    let (sim, _) = bare();
    sim.register("litter", |inv| {
        for n in 0..10_000 {
            if inv.fs().write(&format!("out/{n}"), "").is_err() {
                break;
            }
        }
        Ok(SimOutput::success())
    });
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let error = exec(&sim, &ctx, &handle, exec_of("litter", &[]))
        .await
        .unwrap_err();
    assert_eq!(error.full_code(), "limit_exceeded.disk");
}

#[tokio::test]
async fn after_a_violation_a_sandbox_answers_poisoned_to_exec_and_collect() {
    let (sim, _) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    sim.inject(SandboxStep::Exec, SimFault::violation("escape"))
        .unwrap();
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req())
            .await
            .map(|_| ())
            .unwrap_err()
            .code(),
        "isolation_violation"
    );
    let poisoned = Err(SandboxError::InvalidState {
        from: SandboxState::Poisoned,
    });
    assert_eq!(
        exec(&sim, &ctx, &handle, exec_req()).await.map(|_| ()),
        poisoned
    );
    assert_eq!(
        collect(&sim, &ctx, &handle, collect_req())
            .await
            .map(|_| ()),
        poisoned
    );
    assert_eq!(sim.program_calls(), 0);
    destroy(&sim, &ctx, &handle).await.unwrap();
    assert_eq!(sim.live_sandboxes(), 0);

    // A violation raised by the program itself poisons the sandbox the same way.
    let ctx2 = ctx_for("tenant-a", "node-2");
    sim.register("escape", |_| Err(SimFault::violation("escape")));
    let (handle, _) = create(&sim, &ctx2, create_req()).await;
    assert!(exec(&sim, &ctx2, &handle, exec_of("escape", &[]))
        .await
        .is_err());
    assert_eq!(
        collect(&sim, &ctx2, &handle, collect_req())
            .await
            .map(|_| ()),
        poisoned
    );
}

#[tokio::test]
async fn collect_after_the_ttl_is_not_found() {
    let (sim, clock) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    exec(&sim, &ctx, &handle, exec_req()).await.unwrap();
    clock.advance(60_000);
    assert_eq!(
        collect(&sim, &ctx, &handle, collect_req())
            .await
            .map(|_| ()),
        Err(SandboxError::NotFound)
    );
}

#[tokio::test]
async fn sweep_and_residue_agree_about_an_execution_still_under_way() {
    let (sim, clock) = bare();
    let ctx = ctx();
    let (handle, _) = create(&sim, &ctx, create_req()).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (inside, probe, time) = (Arc::clone(&seen), sim.clone(), Arc::clone(&clock));
    let retention = sim.retention_ms();
    sim.register("long", move |_| {
        // The execution outlives the retention of its own marker.
        time.advance(retention + 1);
        let residue = probe.residue();
        let swept = probe.sweep();
        inside.lock().unwrap().push((residue, swept.records));
        Ok(SimOutput::success())
    });
    // The create record is expired and idle, the exec marker is expired but running.
    exec(&sim, &ctx, &handle, exec_of("long", &[]))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    // The sandbox itself is past its ttl as well: one sandbox plus the create record.
    assert_eq!(
        seen.as_slice(),
        &[(2, 1)],
        "residue counts what a sweep removes"
    );
}
