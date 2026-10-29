//! Proof: the signer, the chain it keeps, the guard that waits for the signature, and the
//! verifier that checks a record against keys it pinned.
//!
//! The guard builds an execution proof from the digests it recomputed, has the target's signer
//! sign it **before** the result is recorded or returned, and returns nothing when the
//! signature is missing, on the first call and on a replay. The signer is idempotent by key and
//! keeps one ordered chain per run. The cases do not prove that a backend told the truth about
//! the content it produced: the proof attests what was declared and what the guard recomputed.

use super::{case, CaseDef};
use crate::conformance::support::{
    ensure, expect_code, fail, join_all, until, BoxFut, Env, Lab, Must, Verdict,
};
use crate::digest::sha256_hex;
use crate::error::SandboxError;
use crate::limits::Limits;
use crate::outcome::DeclaredExit;
use crate::proof::{
    payload_hash, verify_chain, verify_chain_ending_at, verify_chain_for, verify_chain_values,
    verify_proof, verify_proof_value, ChainFault, ExecutionProof, InProcessProofSigner,
    ProofApproval, ProofError, ProofSigner, SignedExecutionProof, SignerError, SignerRefusal,
    TrustedKeys, PROOF_KIND,
};
use crate::{ProvedCollected, SandboxStep};
use serde_json::{json, Value};

pub(super) fn cases() -> Vec<CaseDef> {
    vec![
        case!(
            Proof,
            "proof.signed_and_chained",
            Guard,
            "a collected result comes with a proof that verifies against the pinned keys, states what the guard recomputed and what was approved, and chains to the run's previous proof",
            signed_and_chained
        ),
        case!(
            Proof,
            "proof.chains_are_per_tenant",
            Guard,
            "two tenants that use the same run id each get their own chain: the first proof of each has no previous hash, and neither chain verifies as the other's",
            chains_are_per_tenant
        ),
        case!(
            Proof,
            "proof.tampered_output_invalidates",
            Guard,
            "altering an output changes its digest and invalidates the proof, even when the attacker recomputes the hash",
            tampered_output_invalidates
        ),
        case!(
            Proof,
            "proof.signer_unavailable_fails_closed",
            Guard,
            "an unavailable signer fails the collect with a retriable unavailable error, records and returns nothing, and leaves the sandbox collectable without running the program again",
            signer_unavailable_fails_closed
        ),
        case!(
            Proof,
            "proof.signer_refusal_fails_closed",
            Guard,
            "a signer that refuses fails the collect with a dedicated code, records and returns nothing, and leaves the sandbox collectable",
            signer_refusal_fails_closed
        ),
        case!(
            Proof,
            "proof.no_result_without_signature_on_replay",
            Guard,
            "no call ever returns a result the signer has not signed: not the first collect, not a replay of one that failed, not a replay of one that succeeded",
            no_result_without_signature_on_replay
        ),
        case!(
            Proof,
            "proof.record_must_match_the_submission",
            Guard,
            "a record that is not about the proof the guard submitted is refused and nothing is recorded",
            record_must_match_the_submission
        ),
        case!(
            Proof,
            "proof.coherent_record_for_other_content_refused",
            Guard,
            "a record that is internally coherent (its payload hash is right) but is about another proof than the one submitted is refused and nothing is recorded",
            coherent_record_for_other_content_refused
        ),
        case!(
            Proof,
            "proof.chain_ordered_under_concurrent_calls",
            Guard,
            "proofs signed by concurrent collects of one run form one ordered chain, each exactly once",
            chain_ordered_under_concurrent_calls
        ),
        case!(
            Proof,
            "proof.replayed_collect_same_record",
            Guard,
            "a replayed collect asks the signer again and receives the same record, and the chain gains no second record",
            replayed_collect_same_record
        ),
        case!(
            Proof,
            "proof.restart_replay_same_record",
            Guard,
            "a node replayed by a restarted guard receives the record signed before the restart",
            restart_replay_same_record
        ),
        case!(
            Proof,
            "proof.no_signer_no_proof",
            Guard,
            "a guard without a signer builds no proof, requires none, and refuses to give one",
            no_signer_no_proof
        ),
        case!(
            Proof,
            "proof.signer_idempotent_by_key",
            Guard,
            "a signer asked again for a proof under the same key returns the record already signed, appends nothing, and refuses another content under that key",
            signer_idempotent_by_key
        ),
        case!(
            Proof,
            "proof.signer_refuses_malformed",
            Guard,
            "a signer refuses a proof of another kind or with malformed digests, and the chain is left as it was",
            signer_refuses_malformed
        ),
        case!(
            Proof,
            "proof.kind_signed",
            Guard,
            "the kind of the record is signed: changing it invalidates the record, and the signer signs no other kind",
            kind_signed
        ),
        case!(
            Proof,
            "proof.unpinned_keys_refused",
            Guard,
            "a record verifies only against keys the verifier pinned, by key identifier, with rotation and revocation; a key carried by the record is never used",
            unpinned_keys_refused
        ),
        case!(
            Proof,
            "proof.chain_tamper_detected",
            Guard,
            "a chain with a record reordered, removed, inserted, repeated or cut off its end does not verify",
            chain_tamper_detected
        ),
        case!(
            Proof,
            "proof.mixed_chain_rejected",
            Guard,
            "a chain that mixes sandbox proofs with approval attestations is rejected, wherever the foreign record stands",
            mixed_chain_rejected
        ),
    ]
}

fn sha(label: &str) -> String {
    sha256_hex(label.as_bytes())
}

/// A well-formed proof that no sandbox produced, for the cases that exercise the signer and the
/// verifier alone.
fn synthetic(run: &str, node: &str) -> ExecutionProof {
    ExecutionProof {
        kind: PROOF_KIND.to_owned(),
        run_id: run.to_owned(),
        node_id: node.to_owned(),
        tenant_id: "tenant-a".to_owned(),
        sandbox_id: format!("sb-{node}"),
        idempotency_key: sha(&format!("collect/{run}/{node}")),
        isolation_domain: "dom-1".to_owned(),
        request_digest: sha("request"),
        profile_digest: sha("default"),
        limits_effective: Limits::default(),
        inputs_digest: sha("inputs"),
        outputs_digest: sha("outputs"),
        stdout_digest: sha("stdout"),
        stderr_digest: sha("stderr"),
        truncated: false,
        declared_exit: DeclaredExit::Code { code: 0 },
        declared_duration_ms: 12,
        started_at: 1_000,
        finished_at: 1_050,
        violation: false,
        approval: ProofApproval {
            call_key: format!("sandbox.exec#{}", sha("call")),
            approved_by: "reviewer".to_owned(),
        },
        approval_payload_hash: sha("approval"),
    }
}

/// Put records in chain order by following the `prev_hash` links from the first. Records that
/// do not fit the chain are left out, which a later check on the length reports.
fn in_chain_order(mut records: Vec<SignedExecutionProof>) -> Vec<SignedExecutionProof> {
    let mut ordered = Vec::new();
    let mut prev: Option<String> = None;
    while let Some(at) = records.iter().position(|r| r.prev_hash == prev) {
        let next = records.remove(at);
        prev = Some(next.payload_hash.clone());
        ordered.push(next);
    }
    ordered
}

fn describe<T, E: std::fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => error.to_string(),
    }
}

async fn chains_are_per_tenant(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    // The same run id (the suite uses one) under two tenants, a collect each, interleaved.
    let mut chains: Vec<(&str, Vec<SignedExecutionProof>)> =
        vec![("tenant-a", Vec::new()), ("tenant-b", Vec::new())];
    for node in ["node-1", "node-2"] {
        for (tenant, records) in &mut chains {
            let ctx = env.ctx(tenant, node);
            let (_, record) = env.cycle_proved(&ctx, &exec).await?;
            records.push(record);
        }
    }
    let run = env.ctx("tenant-a", "node-1").run_id;
    for (tenant, records) in &chains {
        ensure!(
            records[0].prev_hash.is_none(),
            "the first proof of {tenant} is chained to a proof of another tenant"
        );
        let verified = verify_chain_for(records, &env.trusted, tenant, run.as_str());
        ensure!(
            verified.is_ok(),
            "the chain of {tenant} does not verify as its own: {verified:?}"
        );
    }
    let (a, b) = (&chains[0].1, &chains[1].1);
    ensure!(
        b[0].prev_hash != Some(a[0].payload_hash.clone())
            && b[1].prev_hash.as_deref() == Some(b[0].payload_hash.as_str()),
        "the chain of tenant-b is linked to the proofs of tenant-a"
    );
    ensure!(
        verify_chain_for(b, &env.trusted, "tenant-a", run.as_str()).is_err(),
        "the chain of tenant-b verifies as the chain of tenant-a"
    );
    Ok(())
}

async fn signed_and_chained(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let mut records = Vec::new();
    for node in ["node-1", "node-2"] {
        let ctx = env.ctx("tenant-a", node);
        let (ran, record) = env.cycle_proved(&ctx, &exec).await?;
        let verified = verify_proof(&record, &env.trusted);
        ensure!(
            verified.is_ok(),
            "the proof of {node} does not verify against the pinned keys: {}",
            describe(&verified)
        );
        let p = &record.proof;
        ensure!(p.kind == PROOF_KIND, "the proof of {node} has another kind");
        ensure!(
            p.tenant_id == "tenant-a" && p.run_id == ctx.run_id.as_str() && p.node_id == node,
            "the proof of {node} names another tenant, run or node"
        );
        ensure!(
            p.sandbox_id == ran.handle.sandbox_id().as_str(),
            "the proof of {node} names another sandbox"
        );
        ensure!(
            p.idempotency_key == Env::key(&ctx, SandboxStep::Collect).as_str(),
            "the proof of {node} is not under the key of the collect"
        );
        ensure!(
            !p.isolation_domain.is_empty(),
            "the proof of {node} names no isolation domain"
        );
        // What the guard recomputed over the bytes it delivered, not what was declared.
        ensure!(
            p.stdout_digest == sha256_hex(&ran.collected.stdout)
                && p.stderr_digest == sha256_hex(&ran.collected.stderr)
                && p.outputs_digest == ran.collected.outputs_digest,
            "the proof of {node} does not carry the digests of the delivered bytes"
        );
        ensure!(
            !ran.collected.stdout.is_empty(),
            "the plain command printed nothing, so the digest check proves little"
        );
        // What was approved.
        ensure!(
            p.approval.call_key == ran.claim.call_key && p.approval.approved_by == "reviewer",
            "the proof of {node} does not state the approval that let the call through"
        );
        ensure!(
            p.approval_payload_hash == "ab".repeat(32),
            "the proof of {node} does not carry the payload hash of the approval"
        );
        ensure!(
            p.declared_exit == ran.outcome.declared_exit
                && p.declared_duration_ms == ran.outcome.declared_duration_ms,
            "the proof of {node} does not carry the declared exit and duration"
        );
        ensure!(
            p.started_at <= p.finished_at && p.finished_at <= env.clock.now_ms(),
            "the proof of {node} is dated out of order"
        );
        ensure!(!p.violation, "the proof of {node} reports a violation");
        records.push(record);
    }
    ensure!(
        records[0].prev_hash.is_none(),
        "the first proof of a run has a predecessor"
    );
    ensure!(
        records[1].prev_hash.as_deref() == Some(records[0].payload_hash.as_str()),
        "the second proof does not chain to the first"
    );
    let chain = verify_chain(&records, &env.trusted);
    ensure!(
        chain.is_ok(),
        "the chain does not verify: {}",
        describe(&chain)
    );
    // Digests only: the proof carries no content of the run.
    let text = serde_json::to_string(&records).unwrap_or_default();
    ensure!(
        !text.contains("hello") && !text.contains("warn"),
        "a proof holds output content"
    );
    Ok(())
}

async fn tampered_output_invalidates(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (ran, record) = env.cycle_proved(&ctx, &exec).await?;
    ensure!(
        verify_proof(&record, &env.trusted).is_ok(),
        "the untouched proof does not verify"
    );
    // The delivered bytes, altered after the fact, no longer have the digest the proof states.
    let mut altered = ran.collected.stdout.clone();
    altered.push(b'!');
    ensure!(
        sha256_hex(&altered) != record.proof.stdout_digest,
        "altering the output left its digest unchanged"
    );
    type Forgery = (&'static str, Box<dyn Fn(&mut SignedExecutionProof) + Send>);
    let forgeries: Vec<Forgery> = vec![
        ("the standard output", {
            let digest = sha256_hex(&altered);
            Box::new(move |r| r.proof.stdout_digest = digest.clone())
        }),
        (
            "the standard error",
            Box::new(|r| r.proof.stderr_digest = sha("forged")),
        ),
        (
            "the files",
            Box::new(|r| r.proof.outputs_digest = sha("forged")),
        ),
    ];
    for (what, forge) in forgeries {
        let mut forged = record.clone();
        forge(&mut forged);
        ensure!(
            verify_proof(&forged, &env.trusted).is_err(),
            "a proof whose digest of {what} was replaced still verifies"
        );
        // The attacker also recomputes the hash of the payload: the signature still does not fit.
        forged.payload_hash = payload_hash(&forged.proof).unwrap_or_default();
        ensure!(
            verify_proof(&forged, &env.trusted) == Err(ProofError::BadSignature),
            "a proof of {what} that was re-hashed still verifies"
        );
    }
    Ok(())
}

async fn signer_unavailable_fails_closed(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;

    signer.fail_next(SignerError::Unavailable);
    let proved = env.gather_proved(&ctx, &handle).await;
    ensure!(
        matches!(&proved, Err(SandboxError::Unavailable { retriable: true })),
        "an unavailable signer: expected a retriable unavailable error, got {}",
        match &proved {
            Ok(_) => "a result".to_owned(),
            Err(error) => error.full_code().to_owned(),
        }
    );
    ensure!(
        matches!(&proved, Err(e) if e.code() == "unavailable" && e.is_retriable()),
        "the error of an unavailable signer is not the retriable unavailable code"
    );
    // The same through the plain collect, which signs too.
    signer.fail_next(SignerError::Unavailable);
    let plain = env.gather(&ctx, &handle).await;
    ensure!(
        matches!(&plain, Err(SandboxError::Unavailable { retriable: true })),
        "the plain collect returned a result the signer did not sign"
    );
    ensure!(
        !env.kinds()
            .contains(&crate::journal::SandboxEventKind::SandboxCollected),
        "a collect that could not be signed was journaled as collected"
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the program was run again"
    );
    // The sandbox is still collectable, and the program still ran once.
    let again = env
        .gather_proved(&ctx, &handle)
        .await
        .must("collect once the signer is back")?;
    ensure!(
        verify_proof(&again.proof, &env.trusted).is_ok(),
        "the proof signed after the outage does not verify"
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the retry ran the program again"
    );
    if let Some(runs) = env.hooks.executions().await {
        ensure!(runs == 1, "the program ran {runs} times");
    }
    Ok(())
}

async fn signer_refusal_fails_closed(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
    for refusal in [
        SignerRefusal::Rejected,
        SignerRefusal::InvalidProof,
        SignerRefusal::Conflict,
    ] {
        signer.fail_next(SignerError::Refused(refusal));
        let result = env.gather_proved(&ctx, &handle).await;
        expect_code(&result, "policy_denied.proof_refused", "a refusing signer")?;
        ensure!(
            matches!(&result, Err(e) if !e.is_retriable()),
            "a refusal by the signer is retriable"
        );
    }
    ensure!(
        !env.kinds()
            .contains(&crate::journal::SandboxEventKind::SandboxCollected),
        "a collect the signer refused was journaled as collected"
    );
    let denied = env
        .journal
        .events()
        .iter()
        .any(|e| e.code.as_deref() == Some("policy_denied.proof_refused"));
    ensure!(denied, "the refusal was not journaled as a denial");
    let again = env
        .gather_proved(&ctx, &handle)
        .await
        .must("collect after the refusals")?;
    ensure!(
        verify_proof(&again.proof, &env.trusted).is_ok(),
        "the proof signed after the refusals does not verify"
    );
    ensure!(
        env.tap.count(SandboxStep::Exec) == 1,
        "the program was run again"
    );
    Ok(())
}

async fn no_result_without_signature_on_replay(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;

    // The signer fails twice before it signs: the first collect and its replay both fail, so
    // a result that failed to be signed is not served from the registry afterwards.
    signer.fail_next(SignerError::Unavailable);
    signer.fail_next(SignerError::Unavailable);
    let first = env.gather_proved(&ctx, &handle).await;
    ensure!(first.is_err(), "a result was returned without a signature");
    let replay = env.gather(&ctx, &handle).await;
    ensure!(
        replay.is_err(),
        "the replay of a collect that could not be signed returned an unsigned result"
    );
    let signed = env
        .gather_proved(&ctx, &handle)
        .await
        .must("collect once the signer answers")?;

    // And after a success, a replay is signed again: with the signer down it returns nothing.
    signer.fail_next(SignerError::Unavailable);
    let replay = env.gather_proved(&ctx, &handle).await;
    ensure!(
        replay.is_err(),
        "a replayed collect returned a result while the signer was down"
    );
    let replay = env
        .gather_proved(&ctx, &handle)
        .await
        .must("replay once the signer answers")?;
    ensure!(
        replay.proof == signed.proof,
        "the replay did not return the record signed first"
    );
    Ok(())
}

async fn record_must_match_the_submission(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
    signer.rewrite_next(|mut record| {
        record.proof.stdout_digest = sha256_hex(b"something else");
        record
    });
    let result = env.gather_proved(&ctx, &handle).await;
    expect_code(
        &result,
        "policy_denied.proof_refused",
        "a record that says another digest",
    )?;
    ensure!(
        !env.kinds()
            .contains(&crate::journal::SandboxEventKind::SandboxCollected),
        "a result was recorded under a record that was not about it"
    );
    env.gather_proved(&ctx, &handle)
        .await
        .must("collect afterwards")?;
    Ok(())
}

async fn coherent_record_for_other_content_refused(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
    // The signer answers for another output, with a payload hash that matches what it holds.
    signer.rewrite_next(|mut record| {
        record.proof.stdout_digest = sha256_hex(b"something else");
        record.payload_hash = payload_hash(&record.proof).unwrap_or_default();
        record
    });
    let result = env.gather_proved(&ctx, &handle).await;
    expect_code(
        &result,
        "policy_denied.proof_refused",
        "a coherent record for another output",
    )?;
    // The same for the approval the proof states.
    signer.rewrite_next(|mut record| {
        record.proof.approval_payload_hash = sha256_hex(b"another approval");
        record.payload_hash = payload_hash(&record.proof).unwrap_or_default();
        record
    });
    let result = env.gather_proved(&ctx, &handle).await;
    expect_code(
        &result,
        "policy_denied.proof_refused",
        "a coherent record for another approval",
    )?;
    ensure!(
        !env.kinds()
            .contains(&crate::journal::SandboxEventKind::SandboxCollected),
        "a result was recorded under a record that was not about it"
    );
    env.gather_proved(&ctx, &handle)
        .await
        .must("collect afterwards")?;
    Ok(())
}

async fn chain_ordered_under_concurrent_calls(lab: &Lab<'_>) -> Verdict {
    // The default policy lets a tenant hold four sandboxes at once.
    const NODES: usize = 4;
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let mut sandboxes = Vec::new();
    for index in 0..NODES {
        let ctx = env.ctx("tenant-a", &format!("node-{index}"));
        let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
        sandboxes.push((ctx, handle));
    }
    // Every collect reaches the signer before any of them is let through.
    let gate = signer.hold();
    let mut calls: Vec<BoxFut<'_, Result<ProvedCollected, SandboxError>>> = sandboxes
        .iter()
        .map(|(ctx, handle)| {
            Box::pin(env.gather_proved(ctx, handle))
                as BoxFut<'_, Result<ProvedCollected, SandboxError>>
        })
        .collect();
    for (index, call) in calls.iter_mut().enumerate() {
        let early = until(call, || gate.arrivals() > index).await;
        ensure!(
            early.is_none(),
            "a collect finished before the signer let it through"
        );
    }
    gate.open();
    // Let the calls run side by side, as concurrent callers would.
    let mut records = Vec::new();
    for answer in join_all(&mut calls).await {
        records.push(answer.must("a concurrent collect")?.proof);
    }
    let ordered = in_chain_order(records.clone());
    ensure!(
        ordered.len() == NODES,
        "the {NODES} proofs do not form one chain ({} link up)",
        ordered.len()
    );
    let chain = verify_chain(&ordered, &env.trusted);
    ensure!(
        chain.is_ok(),
        "the chain of concurrent proofs does not verify: {}",
        describe(&chain)
    );
    let mut payloads: Vec<_> = records.iter().map(|r| r.payload_hash.clone()).collect();
    payloads.sort();
    payloads.dedup();
    ensure!(
        payloads.len() == NODES,
        "a proof was signed twice or two proofs are alike"
    );
    ensure!(
        signer.distinct().len() == NODES,
        "the signer signed more records than collects"
    );
    Ok(())
}

async fn replayed_collect_same_record(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
    let first = env
        .gather_proved(&ctx, &handle)
        .await
        .must("collect")?
        .proof;
    let again = env
        .gather_proved(&ctx, &handle)
        .await
        .must("replayed collect")?
        .proof;
    ensure!(first == again, "a replayed collect returned another record");
    ensure!(
        signer.calls() == 2,
        "the signer was asked {} times; a replayed collect asks again",
        signer.calls()
    );
    ensure!(
        env.tap.count(SandboxStep::Collect) == 1,
        "the replay reached the backend"
    );
    // The chain gained no second record: the next node chains to the first.
    let next = env.ctx("tenant-a", "node-2");
    let (_, second) = env.cycle_proved(&next, &exec).await?;
    ensure!(
        second.prev_hash.as_deref() == Some(first.payload_hash.as_str()),
        "the next proof does not chain to the one that was replayed"
    );
    let all = signer.distinct();
    ensure!(
        all.len() == 2,
        "the signer produced {} records for two collects",
        all.len()
    );
    ensure!(
        verify_chain(&[first, second], &env.trusted).is_ok(),
        "the chain after a replay does not verify"
    );
    Ok(())
}

async fn restart_replay_same_record(lab: &Lab<'_>) -> Verdict {
    let env = lab.signed_env().await?;
    let exec = Env::success(lab)?;
    let signer = env.tapped_signer()?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (ran, first) = env.cycle_proved(&ctx, &exec).await?;
    // The host restarts some time later and replays the node with the same claim.
    env.hooks.advance_clock(5_000).await;
    let guard = env.restarted_signed();
    let start = env.run_started();
    let never = Env::never();
    let create = env.create_req();
    let handle = guard
        .create(&ctx, create, &exec, Some(&ran.claim), start, &never)
        .await
        .must("replayed create")?;
    guard
        .exec(&ctx, &handle, exec.clone(), Some(&ran.claim), start, &never)
        .await
        .must("replayed exec")?;
    let replayed = guard
        .collect_proved(&ctx, &handle, env.collect_req(), start, &never)
        .await
        .must("replayed collect")?;
    ensure!(
        replayed.proof == first.clone(),
        "the replayed node received another record"
    );
    ensure!(
        signer.distinct().len() == 1,
        "the replay added a record to the chain"
    );
    Ok(())
}

async fn no_signer_no_proof(lab: &Lab<'_>) -> Verdict {
    let env = lab.env().await;
    let exec = Env::success(lab)?;
    let ctx = env.ctx("tenant-a", "node-1");
    let (handle, _) = env.executed(&ctx, env.create_req(), &exec).await?;
    let result = env.gather_proved(&ctx, &handle).await;
    ensure!(
        matches!(&result, Err(SandboxError::Unavailable { retriable: false })),
        "a guard without a signer: expected a non-retriable unavailable error, got {}",
        match &result {
            Ok(_) => "a proof".to_owned(),
            Err(error) => error.full_code().to_owned(),
        }
    );
    ensure!(
        env.tap.count(SandboxStep::Collect) == 0,
        "the refusal to give a proof reached the backend"
    );
    // Without a signer, a collect is what it always was.
    let collected = env.gather(&ctx, &handle).await.must("plain collect")?;
    ensure!(
        !collected.stdout.is_empty(),
        "the plain collect delivered nothing"
    );
    Ok(())
}

async fn signer_idempotent_by_key(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let signer = &kit.signer;
    let first_proof = synthetic("run-1", "node-1");
    let first = signer.sign(&first_proof).await.must_sign("a first proof")?;
    ensure!(
        first.prev_hash.is_none(),
        "the first record has a predecessor"
    );
    let again = signer
        .sign(&first_proof)
        .await
        .must_sign("the same proof")?;
    ensure!(
        again == first,
        "the same proof was signed into another record"
    );

    // The same execution dated again (a host that restarted): the first record stands.
    let mut later = first_proof.clone();
    later.started_at += 700;
    later.finished_at += 700;
    let dated = signer
        .sign(&later)
        .await
        .must_sign("the same proof, dated again")?;
    ensure!(
        dated == first,
        "a proof dated again was signed into another record"
    );

    // Replays do not extend the chain.
    let second = signer
        .sign(&synthetic("run-1", "node-2"))
        .await
        .must_sign("a second proof")?;
    ensure!(
        second.prev_hash.as_deref() == Some(first.payload_hash.as_str()),
        "the second record does not chain to the first"
    );
    let again = signer
        .sign(&first_proof)
        .await
        .must_sign("the first proof again")?;
    ensure!(
        again == first,
        "the first proof was signed again after the second"
    );
    let third = signer
        .sign(&synthetic("run-1", "node-3"))
        .await
        .must_sign("a third proof")?;
    ensure!(
        third.prev_hash.as_deref() == Some(second.payload_hash.as_str()),
        "a replay put a record in the chain"
    );
    // One chain per run: another run starts its own.
    let other = signer
        .sign(&synthetic("run-2", "node-1"))
        .await
        .must_sign("a proof of another run")?;
    ensure!(
        other.prev_hash.is_none(),
        "a run continued another run's chain"
    );

    // Another content under a signed key is refused, never signed.
    let mut altered = first_proof;
    altered.stdout_digest = sha("something else");
    let result = signer.sign(&altered).await;
    ensure!(
        matches!(result, Err(SignerError::Refused(_))),
        "a proof with another content was signed under a key already signed"
    );
    let chain = verify_chain(&[first, second, third], &kit.trusted);
    ensure!(
        chain.is_ok(),
        "the chain of the signer does not verify: {}",
        describe(&chain)
    );
    Ok(())
}

/// `Must`-like conversion for the signer's own error, which is not a `SandboxError`.
trait MustSign<T> {
    fn must_sign(self, what: &str) -> Result<T, crate::conformance::support::Stop>;
}

impl<T> MustSign<T> for Result<T, SignerError> {
    fn must_sign(self, what: &str) -> Result<T, crate::conformance::support::Stop> {
        self.map_err(|error| {
            crate::conformance::support::Stop::Fail(format!("{what}: the signer answered {error}"))
        })
    }
}

async fn signer_refuses_malformed(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let signer = &kit.signer;
    let first = signer
        .sign(&synthetic("run-1", "node-1"))
        .await
        .must_sign("a first proof")?;
    type Spoil = (&'static str, Box<dyn Fn(&mut ExecutionProof) + Send>);
    let spoiled: Vec<Spoil> = vec![
        ("another kind", Box::new(|p| p.kind = "approval".to_owned())),
        ("an empty kind", Box::new(|p| p.kind.clear())),
        (
            "a digest that is not a SHA-256",
            Box::new(|p| p.stdout_digest = "abc".to_owned()),
        ),
        (
            "an approval hash that is not a SHA-256",
            Box::new(|p| p.approval_payload_hash = "zz".repeat(32)),
        ),
        ("an empty tenant", Box::new(|p| p.tenant_id = String::new())),
        (
            "an execution that ended before it began",
            Box::new(|p| p.finished_at = p.started_at.saturating_sub(1)),
        ),
    ];
    for (what, spoil) in spoiled {
        let mut proof = synthetic("run-1", &format!("spoiled-{what}"));
        spoil(&mut proof);
        let result = signer.sign(&proof).await;
        ensure!(
            matches!(result, Err(SignerError::Refused(_))),
            "a proof with {what} was signed"
        );
    }
    // The chain is as it was.
    let next = signer
        .sign(&synthetic("run-1", "node-2"))
        .await
        .must_sign("a proof after the refusals")?;
    ensure!(
        next.prev_hash.as_deref() == Some(first.payload_hash.as_str()),
        "a refused proof left a trace in the chain"
    );
    Ok(())
}

async fn kind_signed(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let signer = &kit.signer;
    let record = signer
        .sign(&synthetic("run-1", "node-1"))
        .await
        .must_sign("a proof")?;
    ensure!(
        verify_proof(&record, &kit.trusted).is_ok(),
        "the untouched record does not verify"
    );
    let json = serde_json::to_value(&record).unwrap_or(Value::Null);
    ensure!(
        json["proof"]["kind"] == PROOF_KIND,
        "the record does not say it is a sandbox execution"
    );
    for kind in ["approval", "", "Sandbox_Execution", "sandbox_execution "] {
        let mut changed = record.clone();
        changed.proof.kind = kind.to_owned();
        ensure!(
            verify_proof(&changed, &kit.trusted).is_err(),
            "a record whose kind was changed to {kind:?} still verifies"
        );
        // Even when the hash of the payload is recomputed, the signature does not cover it.
        changed.payload_hash = payload_hash(&changed.proof).unwrap_or_default();
        ensure!(
            verify_proof(&changed, &kit.trusted).is_err(),
            "a record whose kind was changed to {kind:?} and re-hashed still verifies"
        );
    }
    let mut foreign = synthetic("run-1", "node-2");
    foreign.kind = "approval".to_owned();
    ensure!(
        signer.sign(&foreign).await.is_err(),
        "the signer signed a proof of another kind"
    );
    Ok(())
}

async fn unpinned_keys_refused(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let record = kit
        .signer
        .sign(&synthetic("run-1", "node-1"))
        .await
        .must_sign("a proof")?;
    ensure!(
        verify_proof(&record, &kit.trusted).is_ok(),
        "the record does not verify against the pinned keys"
    );
    ensure!(
        verify_proof(&record, &TrustedKeys::new()) == Err(ProofError::UnknownKey),
        "a record verifies with no key pinned"
    );

    let other = InProcessProofSigner::from_seed("another-key", &[0x42; 32]);
    let mut others = TrustedKeys::new();
    others.trust("another-key", &other.public_key()).ok();
    ensure!(
        verify_proof(&record, &others) == Err(ProofError::UnknownKey),
        "a record verifies under a key identifier the verifier never pinned"
    );
    // The verifier pinned another key under the record's own identifier: the record cannot
    // bring the key that verifies it.
    let mut swapped = TrustedKeys::new();
    swapped.trust(record.kid.clone(), &other.public_key()).ok();
    ensure!(
        verify_proof(&record, &swapped) == Err(ProofError::BadSignature),
        "a record verifies under a key that is not the one that signed it"
    );
    // A record signed by an attacker's key, whatever name it gives itself.
    let forged = InProcessProofSigner::from_seed(record.kid.clone(), &[0x42; 32]);
    let forged = forged
        .sign(&synthetic("run-1", "node-1"))
        .await
        .must_sign("a proof signed by another key")?;
    ensure!(
        verify_proof(&forged, &kit.trusted).is_err(),
        "a record signed by a key the verifier did not pin verifies"
    );
    // A key carried by the record is not part of the format and is never used.
    let mut smuggled = serde_json::to_value(&forged).unwrap_or(Value::Null);
    smuggled["publicKey"] = Value::String(
        other
            .public_key()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    );
    ensure!(
        verify_proof_value(&smuggled, &swapped).is_err()
            && verify_proof_value(&smuggled, &kit.trusted).is_err(),
        "a record that carries its own key verifies"
    );

    // Rotation: both keys valid at once; revocation: the revoked one is not, for good.
    let mut rotated = kit.trusted.clone();
    ensure!(
        rotated.trust("another-key", &other.public_key()).is_ok(),
        "a second key cannot be pinned"
    );
    let signed_by_other = other
        .sign(&synthetic("run-9", "node-1"))
        .await
        .must_sign("a proof of the rotated key")?;
    ensure!(
        verify_proof(&record, &rotated).is_ok() && verify_proof(&signed_by_other, &rotated).is_ok(),
        "records of two pinned keys do not both verify"
    );
    rotated.revoke(&record.kid);
    ensure!(
        verify_proof(&record, &rotated) == Err(ProofError::RevokedKey),
        "a record of a revoked key verifies"
    );
    ensure!(
        verify_proof(&signed_by_other, &rotated).is_ok(),
        "revoking a key invalidated another one"
    );
    Ok(())
}

async fn chain_tamper_detected(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let mut r = Vec::new();
    for node in ["n1", "n2", "n3", "n4"] {
        r.push(
            kit.signer
                .sign(&synthetic("run-1", node))
                .await
                .must_sign("a proof")?,
        );
    }
    let keys = &kit.trusted;
    ensure!(
        verify_chain(&r, keys).is_ok(),
        "the genuine chain does not verify"
    );
    let extra = kit
        .signer
        .sign(&synthetic("run-1", "n5"))
        .await
        .must_sign("a later proof")?;
    let bad: Vec<(&str, Vec<SignedExecutionProof>)> = vec![
        (
            "reordered",
            vec![r[0].clone(), r[2].clone(), r[1].clone(), r[3].clone()],
        ),
        ("without the first", r[1..].to_vec()),
        (
            "without one in the middle",
            vec![r[0].clone(), r[2].clone(), r[3].clone()],
        ),
        (
            "with a record repeated",
            vec![r[0].clone(), r[1].clone(), r[1].clone(), r[2].clone()],
        ),
        (
            "with a record inserted",
            vec![r[0].clone(), extra, r[1].clone()],
        ),
    ];
    for (what, chain) in bad {
        ensure!(
            verify_chain(&chain, keys).is_err(),
            "a chain {what} verifies"
        );
    }
    // Records cut off the end are caught against a head the verifier recorded.
    let head = r[3].payload_hash.clone();
    ensure!(
        verify_chain_ending_at(&r, keys, &head).is_ok(),
        "the genuine chain does not end at its own head"
    );
    let cut = verify_chain_ending_at(&r[..3], keys, &head);
    ensure!(
        matches!(&cut, Err(e) if e.fault == ChainFault::WrongHead),
        "a chain cut off its end verifies against the head"
    );
    // A record of another run does not belong in the chain.
    let foreign = kit
        .signer
        .sign(&synthetic("run-2", "n1"))
        .await
        .must_sign("a proof of another run")?;
    ensure!(
        verify_chain(&[r[0].clone(), foreign], keys).is_err(),
        "a chain with a record of another run verifies"
    );
    Ok(())
}

/// What an approval attestation looks like on the wire: nothing a sandbox verifier accepts.
fn approval_record() -> Value {
    json!({
        "approvalId": "approval-1",
        "runId": "run-1",
        "status": "approved",
        "resolvedBy": "reviewer",
        "subject": "tool:refund",
        "decidedAt": "1000",
        "algorithm": "ed25519",
        "payloadHash": sha("approval view"),
        "prevHash": null,
        "publicKey": "MCowBQYDK2VwAyEA",
        "signature": "AAAA"
    })
}

async fn mixed_chain_rejected(lab: &Lab<'_>) -> Verdict {
    let kit = lab.proof_kit().await?;
    let mut sandbox = Vec::new();
    for node in ["n1", "n2"] {
        let record = kit
            .signer
            .sign(&synthetic("run-1", node))
            .await
            .must_sign("a proof")?;
        sandbox.push(serde_json::to_value(record).unwrap_or(Value::Null));
    }
    let approval = approval_record();
    ensure!(
        verify_chain_values(&sandbox, &kit.trusted).is_ok(),
        "the sandbox chain does not verify as values"
    );
    ensure!(
        verify_proof_value(&approval, &kit.trusted) == Err(ProofError::NotSandboxRecord),
        "an approval attestation was taken for a sandbox record"
    );
    let mixes = [
        vec![approval.clone(), sandbox[0].clone(), sandbox[1].clone()],
        vec![sandbox[0].clone(), approval.clone(), sandbox[1].clone()],
        vec![sandbox[0].clone(), sandbox[1].clone(), approval.clone()],
        vec![approval.clone(), approval.clone()],
    ];
    for (index, mix) in mixes.iter().enumerate() {
        match verify_chain_values(mix, &kit.trusted) {
            Err(error) if matches!(error.fault, ChainFault::Record(_)) => {}
            other => fail!(
                "a chain mixing approval and sandbox records (variant {index}) was not rejected: {}",
                describe(&other)
            ),
        }
    }
    // A sandbox record dressed as another kind is no sandbox record either.
    let mut dressed = sandbox[0].clone();
    dressed["proof"]["kind"] = Value::String("approval".to_owned());
    ensure!(
        verify_proof_value(&dressed, &kit.trusted) == Err(ProofError::WrongKind),
        "a sandbox record of another kind verifies"
    );
    Ok(())
}
