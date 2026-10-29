//! Events: one event per lifecycle transition in the guard's own journal, in the order of the
//! calls, holding identifiers and digests and never content, written once however many times
//! a node is replayed.

use super::{case, CaseDef};
use crate::collected::RawFile;
use crate::conformance::support::{
    ensure, expect_code, fail, shape, until, BoxFut, Env, Lab, Must, Verdict,
};
use crate::digest::sha256_hex;
use crate::error::SandboxError;
use crate::ids::SandboxStep;
use crate::journal::SandboxEventKind;
use crate::outcome::ExecOutcome;
use crate::path::LogicalPath;
use crate::request::ExecRequest;
use std::sync::{Arc, Mutex, PoisonError};

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Events,
            "events.transitions_in_order",
            Guard,
            "each transition is one event with its identifiers, and exec_started precedes the backend call",
            transitions_in_order
        ),
        case!(
            Events,
            "events.deduplicated_on_replay",
            Guard,
            "a node replayed by a restarted host writes no event again",
            deduplicated_on_replay
        ),
        case!(
            Events,
            "events.no_content",
            Guard,
            "events hold identifiers and digests, never an argument, an input or an output",
            no_content
        ),
        case!(
            Events,
            "events.journal_failure_fails_closed",
            Guard,
            "a journal that cannot record fails the call as retriable, before or after the effect",
            journal_failure_fails_closed
        ),
        case!(
            Events,
            "events.not_found_and_busy_not_journaled",
            Guard,
            "a not found and a busy are counted or answered, not journaled one by one",
            not_found_and_busy_not_journaled
        ),
        case!(
            Events,
            "events.denial_carries_its_code",
            Guard,
            "a refusal is journaled once with its full code",
            denial_carries_its_code
        ),
    ]
}

async fn transitions_in_order(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    // At the moment the exec reaches the backend, the journal already says it started.
    let at_backend: Arc<Mutex<Vec<SandboxEventKind>>> = Arc::default();
    let (journal, seen) = (Arc::clone(&env.journal), Arc::clone(&at_backend));
    env.tap.on_exec(Arc::new(move || {
        let kinds = journal.events().iter().map(|event| event.kind).collect();
        *seen.lock().unwrap_or_else(PoisonError::into_inner) = kinds;
    }));
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    let before_exec = at_backend
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    ensure!(
        before_exec
            == [
                SandboxEventKind::SandboxCreated,
                SandboxEventKind::SandboxExecStarted
            ],
        "the journal did not say the exec had started when the backend was asked"
    );
    let events = env.journal.events();
    ensure!(
        events.iter().all(|event| event.tenant_id == ctx.tenant_id
            && event.run_id == ctx.run_id
            && event.node_id == ctx.node_id
            && event.attempt == ctx.attempt
            && event.key.len() == 64),
        "an event does not carry the identity of the call"
    );
    ensure!(
        events
            .iter()
            .all(|event| event.sandbox_id.as_ref() == Some(ran.handle.sandbox_id())),
        "an event does not name the sandbox"
    );
    let created = &events[0];
    ensure!(
        created.isolation_domain.is_some(),
        "the created event does not name the isolation domain"
    );
    ensure!(
        events[2].result_digest.is_some()
            && events[3].result_digest.as_deref() == Some(&ran.collected.outputs_digest),
        "the completed and collected events do not carry the digest of what they produced"
    );
    ensure!(
        events.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms),
        "the events are not in the order of the clock"
    );
    // A failed exec is journaled with its code.
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.tap
        .fail_next(SandboxStep::Exec, SandboxError::Backend("x".to_owned()));
    let failed = env.run(&ctx, &handle, exec, &claim).await;
    expect_code(&failed, "backend_error", "an exec the backend fails")?;
    ensure!(
        env.journal
            .events()
            .iter()
            .any(|event| event.kind == SandboxEventKind::SandboxExecFailed
                && event.code.as_deref() == Some("backend_error")),
        "a failed exec is not journaled with its code"
    );
    Ok(())
}

async fn deduplicated_on_replay(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    env.guard.destroy(&ctx, &ran.handle).await.must("destroy")?;
    let recorded = env.journal.events();
    // The host restarts and replays the node, then destroys again.
    let guard = env.restarted();
    let (_, _) = env
        .replay(&guard, &ctx, env.create_req(), &exec, &ran.claim)
        .await?;
    let handle = guard
        .create(
            &ctx,
            env.create_req(),
            &exec,
            Some(&ran.claim),
            env.run_started(),
            &Env::never(),
        )
        .await
        .must("replayed create")?;
    guard
        .destroy(&ctx, &handle)
        .await
        .must("replayed destroy")?;
    ensure!(
        env.journal.events() == recorded,
        "a replayed node wrote events again"
    );
    Ok(())
}

async fn no_content(lab: &Lab<'_>) -> Verdict {
    const OUTPUT: &[u8] = b"unique-output-token-5f3a";
    const FILE: &[u8] = b"unique-file-token-77b2";
    const ARGUMENT: &str = "--unique-argument-token-3d8f";
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("exec")?;
    // The backend hands back output and a file with tokens nobody else knows.
    env.tap.rewrite_collect(Box::new(|result| {
        result.map(|mut raw| {
            raw.stdout = OUTPUT.to_vec();
            raw.declared_stdout_sha256 = sha256_hex(OUTPUT);
            raw.files.clear();
            if let Ok(path) = LogicalPath::new("out/unique-name-token-b41c.txt") {
                raw.files.push(RawFile {
                    path,
                    declared_sha256: sha256_hex(FILE),
                    bytes: FILE.to_vec(),
                });
            }
            raw
        })
    }));
    env.gather(&ctx, &handle).await.must("collect")?;
    // A refusal event too: a call with an argument no one approved.
    let refused = ExecRequest {
        argv: vec![exec.argv[0].clone(), ARGUMENT.to_owned()],
        ..exec
    };
    let _ = env
        .guard
        .exec(
            &ctx,
            &handle,
            refused,
            None,
            env.run_started(),
            &Env::never(),
        )
        .await;
    env.guard.destroy(&ctx, &handle).await.must("destroy")?;
    let wire = serde_json::to_string(&env.journal.events()).unwrap_or_default();
    for hidden in [
        ARGUMENT,
        "unique-output-token",
        "unique-file-token",
        "unique-name-token",
    ] {
        ensure!(
            !wire.contains(hidden),
            "the journal holds content of the call"
        );
    }
    ensure!(
        wire.contains("sandbox_exec_completed")
            && wire.contains("sandbox_collected")
            && wire.contains("sandbox_denied"),
        "the journal misses events the case relies on"
    );
    Ok(())
}

async fn journal_failure_fails_closed(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let create = env.create_req();

    // Before the effect: no event, no execution.
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, create.clone(), &exec).await?;
    env.journal.set_failing(true);
    let blocked = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        matches!(&blocked, Err(SandboxError::Unavailable { retriable: true })),
        "an exec whose start cannot be journaled: expected a retriable unavailability, got {}",
        shape(&blocked)
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "an exec ran without its start being journaled"
    );
    env.journal.set_failing(false);
    env.run(&ctx, &handle, exec.clone(), &claim)
        .await
        .must("the exec once the journal is back")?;

    // After the effect: the result is kept, and the retry replays it.
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, claim) = env.ready(&ctx, create, &exec).await?;
    let journal = Arc::clone(&env.journal);
    env.tap.on_exec(Arc::new(move || journal.set_failing(true)));
    let lost = env.run(&ctx, &handle, exec.clone(), &claim).await;
    ensure!(
        matches!(&lost, Err(SandboxError::Unavailable { retriable: true })),
        "an exec whose end cannot be journaled: expected a retriable unavailability, got {}",
        shape(&lost)
    );
    let calls = env.tap.count(SandboxStep::Exec);
    env.journal.set_failing(false);
    env.run(&ctx, &handle, exec, &claim)
        .await
        .must("the retry")?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == calls,
        "the retry ran the exec again"
    );
    let completed = env
        .journal
        .events()
        .into_iter()
        .filter(|event| {
            event.kind == SandboxEventKind::SandboxExecCompleted
                && event.sandbox_id.as_ref() == Some(handle.sandbox_id())
        })
        .count();
    ensure!(
        completed == 1,
        "the completion of the exec was journaled {completed} times, expected once"
    );
    Ok(())
}

async fn not_found_and_busy_not_journaled(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let a = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&a, env.create_req(), &exec).await?;
    let events = env.journal.len();
    let b = env.ctx("tenant-b", "node-1");
    for _ in 0..5 {
        let result = env.gather(&b, &handle).await;
        expect_code(&result, "not_found", "a handle of another tenant")?;
    }
    ensure!(env.journal.len() == events, "a not found was journaled");
    ensure!(
        env.guard.not_found_count() == 5,
        "the not found answers were not counted"
    );

    // A busy is an answer, not an event.
    let gate = env.tap.hold(SandboxStep::Exec);
    let mut first: BoxFut<'_, Result<ExecOutcome, SandboxError>> =
        Box::pin(env.run(&a, &handle, exec.clone(), &claim));
    let held = until(&mut first, || gate.arrivals() >= 1).await;
    ensure!(held.is_none(), "the first exec did not wait at the backend");
    let events = env.journal.len();
    for _ in 0..3 {
        let mut call: BoxFut<'_, Result<ExecOutcome, SandboxError>> =
            Box::pin(env.run(&a, &handle, exec.clone(), &claim));
        let Some(second) = until(&mut call, || gate.arrivals() > 1).await else {
            fail!("a concurrent exec waited at the backend instead of being told it was busy");
        };
        ensure!(
            matches!(&second, Err(SandboxError::Busy { .. })),
            "a concurrent exec: expected busy, got {}",
            shape(&second)
        );
    }
    ensure!(env.journal.len() == events, "a busy answer was journaled");
    gate.open();
    (&mut first).await.must("the held exec")?;
    Ok(())
}

async fn denial_carries_its_code(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let selfish = env
        .approve_by(
            &ctx,
            &env.create_req(),
            &exec,
            "agent",
            "agent",
            &"ab".repeat(32),
        )
        .must("approval request")?;
    for _ in 0..2 {
        let _ = env
            .guard
            .create(
                &ctx,
                env.create_req(),
                &exec,
                Some(&selfish),
                env.run_started(),
                &Env::never(),
            )
            .await;
    }
    let denials: Vec<_> = env
        .journal
        .events()
        .into_iter()
        .filter(|event| event.kind == SandboxEventKind::SandboxDenied)
        .collect();
    ensure!(
        denials.len() == 1,
        "the same refusal was journaled {} times",
        denials.len()
    );
    ensure!(
        denials[0].code.as_deref() == Some("policy_denied.self_approval"),
        "the refusal is not journaled with its full code"
    );
    Ok(())
}
