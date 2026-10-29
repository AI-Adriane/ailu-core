//! Governance: nothing runs without an approval bound to the exact call, an agent never
//! approves itself, a call that cannot be read whole or whose text could mislead is refused
//! before anyone is asked, the approver sees the whole call, and the guard relays nothing a
//! backend says in free text.

use super::{case, CaseDef};
use crate::approval::{VerifiedGrant, MAX_REVIEW_CHARS};
use crate::conformance::support::{ensure, expect_code, fail, shape, Env, Lab, Must, Verdict};
use crate::error::{LimitKind, PolicyReason, QuotaKind, SandboxError, SandboxState};
use crate::ids::{SandboxStep, TenantId};
use crate::journal::SandboxEventKind;
use crate::outcome::{DeclaredExit, ExecOutcome, Truncation};
use crate::path::LogicalPath;
use crate::policy::SandboxPolicy;
use crate::request::{
    CreateRequest, EnvGrant, ExecRequest, HostRule, InputRef, NetworkPolicy, SecretRef,
};
use crate::samples;
use crate::ApprovalClaim;
use ailu_graph_core::RunId;
use serde_json::json;

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Governance,
            "governance.approval_required",
            Guard,
            "without an approval nothing is created or run, and the error carries a minimal subject",
            approval_required
        ),
        case!(
            Governance,
            "governance.grant_bound_to_exact_call",
            Guard,
            "an approval unlocks only the exact call it was given for",
            grant_bound_to_exact_call
        ),
        case!(
            Governance,
            "governance.self_approval_refused",
            Guard,
            "an approval given by the principal that requested the call is refused",
            self_approval_refused
        ),
        case!(
            Governance,
            "governance.unverified_claims_refused",
            Guard,
            "a claim is worth only what the verifier says, and a verifier that is down fails closed",
            unverified_claims_refused
        ),
        case!(
            Governance,
            "governance.replay_needs_no_new_approval",
            Guard,
            "a replayed call is served without a new approval check, since it has no new effect",
            replay_needs_no_new_approval
        ),
        case!(
            Governance,
            "governance.unreviewable_call",
            Guard,
            "a call that cannot be shown whole is refused before any approval is asked",
            unreviewable_call
        ),
        case!(
            Governance,
            "governance.unsafe_text",
            Guard,
            "control, escape, bidirectional and invisible characters are refused wherever the approver reads",
            unsafe_text
        ),
        case!(
            Governance,
            "governance.stdin_refused_before_the_backend",
            Guard,
            "a call that carries standard input is refused under approval before anything reaches the backend",
            stdin_refused_before_the_backend
        ),
        case!(
            Governance,
            "governance.subject_shows_whole_call",
            Guard,
            "the approval subject shows the whole call",
            subject_shows_whole_call
        ),
        case!(
            Governance,
            "governance.payload_hash_is_64_hex",
            Guard,
            "an approval whose payload hash is not 64 lowercase hex characters is refused, a good one is journaled",
            payload_hash_is_64_hex
        ),
        case!(
            Governance,
            "governance.backend_text_not_relayed",
            Guard,
            "a backend's free text is never relayed and the errors reserved to the guard are made generic",
            backend_text_not_relayed
        ),
        case!(
            Governance,
            "governance.partial_outcome_bounded",
            Guard,
            "the partial outcome of a killed program is reduced to bounded, plain values",
            partial_outcome_bounded
        ),
    ]
}

fn rule(host: &str, port: u16) -> HostRule {
    HostRule {
        host: host.to_owned(),
        port,
    }
}

/// A policy that lets requests name `api.example.com:443`, to build a call with a network rule.
fn policy_with_rule() -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();
    policy.network_allow = vec![rule("api.example.com", 443)];
    policy
}

/// A call with a secret grant and a network rule, built on the plain command. It is meant to be refused or described, never run.
fn rich_call(env: &Env, base: ExecRequest) -> (CreateRequest, ExecRequest) {
    let mut create = env.create_req();
    create.env = vec![EnvGrant {
        name: "DB_PASSWORD".to_owned(),
        secret_ref: SecretRef("vault/prod/db".to_owned()),
    }];
    create.network = NetworkPolicy::Allow(vec![rule("api.example.com", 443)]);
    let mut exec = base;
    exec.argv.push("--token=s3cr3t-arg".to_owned());
    (create, exec)
}

async fn approval_required(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(policy_with_rule()).await;
    let (create, exec) = rich_call(&env, Env::success(lab)?);
    let ctx = env.ctx("tenant-a", "node-1");
    let refused = env
        .guard
        .create(
            &ctx,
            create.clone(),
            &exec,
            None,
            env.run_started(),
            &Env::never(),
        )
        .await;
    let Err(error) = refused else {
        fail!("a create with no approval was not refused");
    };
    let SandboxError::ApprovalRequired { subject } = &error else {
        fail!(
            "a create with no approval: expected approval_required, got {}",
            error.full_code()
        );
    };
    ensure!(
        env.tap.total() == 0,
        "a create with no approval reached the backend"
    );
    // Only the call key and counts: the full subject is the host's to build.
    let request = env
        .guard
        .request_approval(&ctx, &create, &exec)
        .must("approval request")?;
    ensure!(
        subject.0["callKey"] == request.call_key.as_str(),
        "the error does not carry the call key"
    );
    ensure!(
        subject.0["exec"]["argc"] == exec.argv.len(),
        "the error does not carry the counts of the call"
    );
    ensure!(
        subject.0["exec"].get("argv").is_none(),
        "the error carries the arguments"
    );
    let wire = serde_json::to_string(&error).unwrap_or_default();
    for hidden in [
        "s3cr3t",
        "--token",
        "DB_PASSWORD",
        "vault",
        "api.example.com",
    ] {
        ensure!(!wire.contains(hidden), "the error carries part of the call");
    }
    ensure!(
        !format!("{error:?}").contains("s3cr3t"),
        "the debug output of the error carries part of the call"
    );
    // Asking again does not write the same refusal twice.
    let _ = env
        .guard
        .create(&ctx, create, &exec, None, env.run_started(), &Env::never())
        .await;
    let denied = env
        .kinds()
        .into_iter()
        .filter(|kind| *kind == SandboxEventKind::SandboxDenied)
        .count();
    ensure!(denied == 1, "the same refusal was journaled {denied} times");

    // And at exec, on a sandbox created with an approval.
    let plain = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-2");
    let (handle, _) = env.ready(&ctx, env.create_req(), &plain).await?;
    let unapproved = env
        .guard
        .exec(&ctx, &handle, plain, None, env.run_started(), &Env::never())
        .await;
    expect_code(&unapproved, "approval_required", "an exec with no approval")?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "an exec with no approval ran"
    );
    Ok(())
}

async fn grant_bound_to_exact_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;

    // One detail of the call changes: one more argument.
    let mut changed = exec.clone();
    changed.argv.push("--another".to_owned());
    let refused = env.run(&ctx, &handle, changed.clone(), &claim).await;
    expect_code(
        &refused,
        "approval_required",
        "an exec changed after the approval",
    )?;
    ensure!(env.tap.count(SandboxStep::Exec) == 0, "a changed exec ran");

    // A grant for another plan, or from another tenant, unlocks nothing at create.
    let other = env.ctx("tenant-a", "node-2");
    let stale = env
        .approve(&other, &env.create_req(), &changed)
        .must("approval request")?;
    let foreign = env
        .approve(&env.ctx("tenant-b", "node-2"), &env.create_req(), &exec)
        .must("approval request")?;
    for (grant, what) in [
        (&stale, "a grant for another plan"),
        (&foreign, "a grant of another tenant"),
    ] {
        let result = env
            .guard
            .create(
                &other,
                env.create_req(),
                &exec,
                Some(grant),
                env.run_started(),
                &Env::never(),
            )
            .await;
        expect_code(&result, "approval_required", what)?;
    }

    // A new grant for the changed call unlocks it.
    let grant = env
        .approve(&ctx, &env.create_req(), &changed)
        .must("approval request")?;
    env.run(&ctx, &handle, changed, &grant)
        .await
        .must("the changed exec with its own grant")?;
    Ok(())
}

async fn self_approval_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    for approver in ["agent", "  AGENT "] {
        let selfish = env
            .approve_by(
                &ctx,
                &env.create_req(),
                &exec,
                "agent",
                approver,
                &"ab".repeat(32),
            )
            .must("approval request")?;
        let result = env
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
        ensure!(
            matches!(
                &result,
                Err(SandboxError::PolicyDenied(PolicyReason::SelfApproval))
            ),
            "an approval by the requester: expected policy_denied.self_approval, got {}",
            shape(&result)
        );
    }
    ensure!(
        env.tap.total() == 0,
        "a self-approved create reached the backend"
    );
    // And at exec, on a sandbox created with a good approval.
    let (handle, _good) = env.ready(&ctx, env.create_req(), &exec).await?;
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
    let result = env.run(&ctx, &handle, exec, &selfish).await;
    expect_code(
        &result,
        "policy_denied.self_approval",
        "a self-approved exec",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "a self-approved exec ran"
    );
    Ok(())
}

async fn unverified_claims_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let right = env
        .guard
        .request_approval(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    let create_with = |claim: ApprovalClaim| {
        let (env, ctx, exec) = (&env, &ctx, &exec);
        async move {
            env.guard
                .create(
                    ctx,
                    env.create_req(),
                    exec,
                    Some(&claim),
                    env.run_started(),
                    &Env::never(),
                )
                .await
        }
    };
    let claim = |id: &str, key: &str| ApprovalClaim {
        approval_id: id.to_owned(),
        call_key: key.to_owned(),
    };
    // An approval nobody recorded: a forged claim.
    let forged = create_with(claim("forged", &right.call_key)).await;
    expect_code(&forged, "approval_required", "a forged claim")?;
    // Recorded for another tenant, another run or another call.
    let base = VerifiedGrant {
        approval_id: "apr-x".to_owned(),
        call_key: right.call_key.clone(),
        tenant_id: ctx.tenant_id.clone(),
        run_id: ctx.run_id.clone(),
        requested_by: "agent".to_owned(),
        approved_by: "reviewer".to_owned(),
        approval_payload_hash: "ab".repeat(32),
    };
    for (id, grant) in [
        (
            "other-tenant",
            VerifiedGrant {
                tenant_id: TenantId::new("tenant-b"),
                ..base.clone()
            },
        ),
        (
            "other-run",
            VerifiedGrant {
                run_id: RunId::from("another-run"),
                ..base.clone()
            },
        ),
        (
            "other-call",
            VerifiedGrant {
                call_key: "sandbox.exec#00".to_owned(),
                ..base.clone()
            },
        ),
    ] {
        env.verifier.approve(VerifiedGrant {
            approval_id: id.to_owned(),
            ..grant
        });
        let result = create_with(claim(id, &right.call_key)).await;
        expect_code(&result, "approval_required", id)?;
    }
    // A claim that names a different call than the approval it cites.
    env.verifier.approve(VerifiedGrant {
        approval_id: "mismatch".to_owned(),
        ..base.clone()
    });
    let result = create_with(claim("mismatch", "sandbox.exec#11")).await;
    expect_code(
        &result,
        "approval_required",
        "a claim for another call than its approval",
    )?;
    ensure!(env.tap.total() == 0, "a false claim reached the backend");
    // The verifier down fails closed, and the claim works once it is back.
    env.verifier.approve(base.clone());
    env.verifier.set_unavailable(true);
    let result = create_with(claim("apr-x", &right.call_key)).await;
    ensure!(
        matches!(&result, Err(SandboxError::Unavailable { retriable: true })),
        "a verifier that is down: expected a retriable unavailability, got {}",
        shape(&result)
    );
    env.verifier.set_unavailable(false);
    create_with(claim("apr-x", &right.call_key))
        .await
        .must("the claim once the verifier is back")?;
    Ok(())
}

async fn replay_needs_no_new_approval(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let ran = env.cycle(&ctx, env.create_req(), &exec).await?;
    let handle = env
        .guard
        .create(
            &ctx,
            env.create_req(),
            &exec,
            None,
            env.run_started(),
            &Env::never(),
        )
        .await
        .must("a replayed create with no claim")?;
    ensure!(
        handle == ran.handle,
        "the replayed create returned another handle"
    );
    let outcome = env
        .guard
        .exec(&ctx, &handle, exec, None, env.run_started(), &Env::never())
        .await
        .must("a replayed exec with no claim")?;
    ensure!(
        outcome == ran.outcome,
        "the replayed exec returned another outcome"
    );
    Ok(())
}

async fn unreviewable_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let ctx = env.ctx("tenant-a", "node-1");
    let plain = |argv: Vec<String>| ExecRequest {
        argv,
        cwd: None,
        stdin: None,
    };
    let mut unreadable: Vec<ExecRequest> = samples::unreviewable_argvs()
        .into_iter()
        .map(plain)
        .collect();
    let long = "d".repeat(MAX_REVIEW_CHARS + 1);
    if let Ok(cwd) = LogicalPath::new(&long) {
        unreadable.push(ExecRequest {
            cwd: Some(cwd),
            ..plain(vec!["tool".to_owned()])
        });
    }
    for exec in unreadable {
        let asked = env.guard.request_approval(&ctx, &env.create_req(), &exec);
        expect_code(
            &asked,
            "policy_denied.unreviewable_call",
            "an approval request for an unreadable call",
        )?;
        // The same refusal at create, with no approval at all.
        let created = env
            .guard
            .create(
                &ctx,
                env.create_req(),
                &exec,
                None,
                env.run_started(),
                &Env::never(),
            )
            .await;
        expect_code(
            &created,
            "policy_denied.unreviewable_call",
            "a create of an unreadable call",
        )?;
    }
    // A field shown to the approver that is too long.
    let mut inputs = env.create_req();
    let input = json!({ "artifact": { "id": "a", "version": 1 }, "dest": "d".repeat(300) });
    let Ok(input) = serde_json::from_value::<InputRef>(input) else {
        fail!("an input destination of 300 characters is not a valid input");
    };
    inputs.inputs.push(input);
    let asked = env
        .guard
        .request_approval(&ctx, &inputs, &plain(vec!["tool".to_owned()]));
    expect_code(
        &asked,
        "policy_denied.unreviewable_call",
        "an input destination too long to be shown",
    )?;
    ensure!(
        env.tap.total() == 0,
        "an unreadable call reached the backend"
    );
    // At exec, on a live sandbox, with a valid approval for the plain call.
    let exec = Env::success(lab)?;
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let too_long = plain(vec!["sh".to_owned(), "x".repeat(300)]);
    let result = env
        .guard
        .exec(
            &ctx,
            &handle,
            too_long,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(
        &result,
        "policy_denied.unreviewable_call",
        "an exec with an argument too long to be shown",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "an unreadable exec ran"
    );
    // Exactly at the limits is accepted.
    let edge = plain((0..32).map(|_| "y".repeat(MAX_REVIEW_CHARS)).collect());
    env.guard
        .request_approval(&ctx, &env.create_req(), &edge)
        .must("a call exactly at the review limits")?;
    Ok(())
}

async fn unsafe_text(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let ctx = env.ctx("tenant-a", "node-1");
    let with_arg = |text: &str| ExecRequest {
        argv: vec!["echo".to_owned(), text.to_owned()],
        cwd: None,
        stdin: None,
    };
    for bad in samples::MISLEADING_TEXT {
        let asked = env
            .guard
            .request_approval(&ctx, &env.create_req(), &with_arg(bad));
        expect_code(
            &asked,
            "policy_denied.unsafe_text",
            "misleading text in an argument",
        )?;
    }
    // The other fields the approver reads.
    let plain = with_arg("ok");
    if let Ok(cwd) = LogicalPath::new("w\u{202e}x") {
        let asked = env.guard.request_approval(
            &ctx,
            &env.create_req(),
            &ExecRequest {
                cwd: Some(cwd),
                ..plain.clone()
            },
        );
        expect_code(
            &asked,
            "policy_denied.unsafe_text",
            "misleading text in the working directory",
        )?;
    }
    let mut grant = env.create_req();
    grant.env.push(EnvGrant {
        name: "A".to_owned(),
        secret_ref: SecretRef("ref\u{202e}".to_owned()),
    });
    let asked = env.guard.request_approval(&ctx, &grant, &plain);
    expect_code(
        &asked,
        "policy_denied.unsafe_text",
        "misleading text in a secret reference",
    )?;
    let mut profile = env.create_req();
    profile.profile = "default\u{200b}".to_owned();
    let asked = env.guard.request_approval(&ctx, &profile, &plain);
    expect_code(
        &asked,
        "policy_denied.unsafe_text",
        "an invisible character in the profile",
    )?;
    // The same at create, before any approval is asked, and nothing reaches the backend.
    let created = env
        .guard
        .create(
            &ctx,
            env.create_req(),
            &with_arg("x\u{1b}[2J"),
            None,
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(
        &created,
        "policy_denied.unsafe_text",
        "a create with an escape sequence",
    )?;
    ensure!(
        env.tap.total() == 0,
        "a call with misleading text reached the backend"
    );
    // A forged marker is no issue: arguments are a structured array, shown whole.
    let ok = env
        .guard
        .request_approval(
            &ctx,
            &env.create_req(),
            &with_arg("ok [0 characters hidden]"),
        )
        .must("a call with plain text that looks like a marker")?;
    ensure!(
        ok.subject.0["exec"]["argv"][1] == "ok [0 characters hidden]",
        "the argument is not shown as it was written"
    );
    Ok(())
}

async fn stdin_refused_before_the_backend(lab: &Lab<'_>) -> Verdict {
    const INPUT: &[u8] = b"unique-stdin-payload-31c8";
    let env = lab.env().await;
    let plain = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let create = env.create_req();
    // The approver is shown no content for a standard input, so nothing covers one: it is
    // refused, empty or not, at every step that takes the call.
    for (what, stdin) in [
        ("a standard input", INPUT.to_vec()),
        ("an empty one", vec![]),
    ] {
        let mut with_stdin = plain.clone();
        with_stdin.stdin = Some(stdin);
        let asked = env.guard.request_approval(&ctx, &create, &with_stdin);
        expect_code(
            &asked,
            "policy_denied.stdin_not_allowed",
            &format!("an approval request with {what}"),
        )?;
        let claim = env
            .approve(&ctx, &create, &plain)
            .must("approval request")?;
        let created = env
            .guard
            .create(
                &ctx,
                create.clone(),
                &with_stdin,
                Some(&claim),
                env.run_started(),
                &Env::never(),
            )
            .await;
        expect_code(
            &created,
            "policy_denied.stdin_not_allowed",
            &format!("a create planning {what}"),
        )?;
        ensure!(
            env.tap.total() == 0,
            "a create planning {what} reached the backend"
        );
    }
    // On a sandbox created with a good approval, the exec is refused the same way, and the
    // sandbox is left as it was: the plain call still runs.
    let (handle, claim) = env.ready(&ctx, create, &plain).await?;
    let mut with_stdin = plain.clone();
    with_stdin.stdin = Some(INPUT.to_vec());
    let refused = env.run(&ctx, &handle, with_stdin, &claim).await;
    expect_code(
        &refused,
        "policy_denied.stdin_not_allowed",
        "an exec with a standard input",
    )?;
    ensure!(
        env.tap.count(SandboxStep::Exec) == 0,
        "an exec with a standard input reached the backend"
    );
    env.run(&ctx, &handle, plain, &claim)
        .await
        .must("the plain exec after the refusal")?;
    let wire = serde_json::to_string(&env.journal.events()).unwrap_or_default();
    ensure!(
        !wire.contains("unique-stdin-payload"),
        "the journal holds the standard input"
    );
    Ok(())
}

async fn subject_shows_whole_call(lab: &Lab<'_>) -> Verdict {
    let env = lab.env_with(policy_with_rule()).await;
    let (create, mut exec) = rich_call(&env, Env::success(lab)?);
    let mut create = create;
    let Ok(dest) = LogicalPath::new("in/data.txt") else {
        fail!("a plain destination is not a valid path");
    };
    create.inputs.push(InputRef {
        artifact: ailu_fs_backend::ArtifactRef {
            id: "conformance-artifact".into(),
            version: 1,
        },
        dest,
    });
    let long = "x".repeat(MAX_REVIEW_CHARS);
    exec.argv.push(long.clone());
    let ctx = env.ctx("tenant-a", "node-1");
    let request = env
        .guard
        .request_approval(&ctx, &create, &exec)
        .must("approval request")?;
    let shown = &request.subject.0;
    let argv = shown["exec"]["argv"].as_array().map_or(0, Vec::len);
    ensure!(
        argv == exec.argv.len(),
        "the subject shows {argv} arguments of {}",
        exec.argv.len()
    );
    ensure!(
        shown["exec"]["argv"][exec.argv.len() - 1] == long.as_str(),
        "an argument is not shown whole"
    );
    ensure!(
        shown["create"]["env"][0]["name"] == "DB_PASSWORD"
            && shown["create"]["env"][0]["secretRef"] == "vault/prod/db",
        "the subject does not show the secret grant"
    );
    ensure!(
        shown["create"]["network"]["rules"][0]["host"] == "api.example.com",
        "the subject does not show the network rule"
    );
    ensure!(
        shown["create"]["inputs"][0]["dest"] == "in/data.txt",
        "the subject does not show the input destination"
    );
    ensure!(
        shown["create"]["ttlMs"] == create.ttl_ms && shown["create"].get("limits").is_some(),
        "the subject does not show the limits and the ttl"
    );
    // There is no standard input to show: a call that carries one is refused before this point.
    ensure!(
        shown["exec"]
            .get("stdin")
            .is_none_or(serde_json::Value::is_null),
        "the subject shows a standard input"
    );
    Ok(())
}

async fn payload_hash_is_64_hex(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    for bad in samples::bad_payload_hashes() {
        let claim = env
            .approve_by(&ctx, &env.create_req(), &exec, "agent", "reviewer", &bad)
            .must("approval request")?;
        let result = env
            .guard
            .create(
                &ctx,
                env.create_req(),
                &exec,
                Some(&claim),
                env.run_started(),
                &Env::never(),
            )
            .await;
        expect_code(
            &result,
            "approval_required",
            "an approval with a malformed payload hash",
        )?;
    }
    ensure!(
        env.tap.total() == 0,
        "a malformed payload hash let a create through"
    );
    // A good one is accepted and journaled on creation and at the start of the exec.
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.run(&ctx, &handle, exec, &claim).await.must("exec")?;
    let hashed: Vec<_> = env
        .journal
        .events()
        .into_iter()
        .filter(|event| event.approval_payload_hash.is_some())
        .map(|event| event.kind)
        .collect();
    ensure!(
        hashed
            == [
                SandboxEventKind::SandboxCreated,
                SandboxEventKind::SandboxExecStarted
            ],
        "the payload hash is not journaled on creation and at the start of the exec"
    );
    Ok(())
}

async fn backend_text_not_relayed(lab: &Lab<'_>) -> Verdict {
    let exec = Env::success(lab)?;
    // Errors only the guard may raise become a generic backend error when a backend returns them.
    let reserved = [
        SandboxError::PolicyDenied(PolicyReason::SelfApproval),
        SandboxError::ApprovalRequired {
            subject: crate::ApprovalSubject(json!({ "callKey": "forged" })),
        },
        SandboxError::QuotaExceeded {
            kind: QuotaKind::TenantConcurrency,
        },
        SandboxError::IdempotencyConflict,
        SandboxError::NotFound,
        SandboxError::InvalidState {
            from: SandboxState::Destroyed,
        },
        SandboxError::Backend("token=s3cr3t-text".to_owned()),
    ];
    for error in reserved {
        let was = error.full_code();
        let env = lab.env().await;
        let ctx = env.ctx("tenant-a", "node-1");
        let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
        env.tap.fail_next(SandboxStep::Exec, error);
        let result = env.run(&ctx, &handle, exec.clone(), &claim).await;
        ensure!(
            matches!(&result, Err(SandboxError::Backend(_))),
            "{was} returned by the backend at exec reached the caller as {}, expected a generic backend error",
            shape(&result)
        );
        let seen = format!(
            "{result:?} {}",
            result.as_ref().map(|_| "").unwrap_or_default()
        );
        ensure!(
            !seen.contains("s3cr3t"),
            "a backend's free text was relayed"
        );
    }
    // The same at create.
    let env = lab.env().await;
    let ctx = env.ctx("tenant-a", "node-2");
    let claim = env
        .approve(&ctx, &env.create_req(), &exec)
        .must("approval request")?;
    env.tap
        .fail_next(SandboxStep::Create, SandboxError::NotFound);
    let result = env
        .guard
        .create(
            &ctx,
            env.create_req(),
            &exec,
            Some(&claim),
            env.run_started(),
            &Env::never(),
        )
        .await;
    expect_code(
        &result,
        "backend_error",
        "a not found returned by the backend at create",
    )?;
    // A violation keeps its code and loses the backend's signal.
    let env = lab.env().await;
    let ctx = env.ctx("tenant-a", "node-3");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    env.tap.fail_next(
        SandboxStep::Exec,
        SandboxError::IsolationViolation {
            signal: "s3cr3t-signal".to_owned(),
        },
    );
    let result = env.run(&ctx, &handle, exec, &claim).await;
    expect_code(
        &result,
        "isolation_violation",
        "a violation reported by the backend",
    )?;
    ensure!(
        !format!("{result:?}").contains("s3cr3t"),
        "the signal of the backend was relayed"
    );
    Ok(())
}

async fn partial_outcome_bounded(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, claim) = env.ready(&ctx, env.create_req(), &exec).await?;
    let wild = ExecOutcome {
        declared_exit: DeclaredExit::Signal {
            name: format!("S3CR3T {}\n", "a".repeat(1000)),
        },
        truncated: true,
        truncation: vec![
            Truncation::Stdout,
            Truncation::Stderr,
            Truncation::Stdout,
            Truncation::Stderr,
        ],
        declared_duration_ms: 1,
    };
    env.tap.rewrite_exec(Box::new(move |_| {
        Err(SandboxError::LimitExceeded {
            kind: LimitKind::WallTime,
            partial: Box::new(wild),
        })
    }));
    let result = env.run(&ctx, &handle, exec, &claim).await;
    let Err(SandboxError::LimitExceeded { partial, .. }) = &result else {
        fail!(
            "a killed program: expected limit_exceeded, got {}",
            shape(&result)
        );
    };
    ensure!(
        partial.truncation.len() <= 2,
        "the partial outcome lists {} truncation markers",
        partial.truncation.len()
    );
    if let DeclaredExit::Signal { name } = &partial.declared_exit {
        ensure!(
            name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "the signal name of the partial outcome is not a short plain word"
        );
    }
    Ok(())
}
