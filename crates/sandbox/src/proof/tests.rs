//! Tests of the proof types, the in-process signer and the verifier.

use super::*;
use crate::digest::sha256_hex;
use serde_json::{json, Value};

pub(crate) const SEED: [u8; 32] = [7; 32];

pub(crate) fn digest(label: &str) -> String {
    sha256_hex(label.as_bytes())
}

pub(crate) fn proof(run: &str, node: &str) -> ExecutionProof {
    ExecutionProof {
        kind: PROOF_KIND.to_owned(),
        run_id: run.to_owned(),
        node_id: node.to_owned(),
        tenant_id: "tenant-a".to_owned(),
        sandbox_id: format!("sb-{node}"),
        idempotency_key: digest(&format!("collect/{run}/{node}")),
        isolation_domain: "dom-1".to_owned(),
        request_digest: digest("request"),
        profile_digest: digest("default"),
        limits_effective: crate::Limits::default(),
        inputs_digest: digest("inputs"),
        outputs_digest: digest("outputs"),
        stdout_digest: digest("stdout"),
        stderr_digest: digest("stderr"),
        truncated: false,
        declared_exit: crate::DeclaredExit::Code { code: 0 },
        declared_duration_ms: 12,
        started_at: 1_000,
        finished_at: 1_050,
        violation: false,
        approval: ProofApproval {
            call_key: format!("sandbox.exec#{}", digest("call")),
            approved_by: "reviewer".to_owned(),
        },
        approval_payload_hash: digest("approval"),
    }
}

pub(crate) fn signer() -> InProcessProofSigner {
    InProcessProofSigner::from_seed("k1", &SEED)
}

pub(crate) fn trusted(signer: &InProcessProofSigner) -> TrustedKeys {
    let mut keys = TrustedKeys::new();
    keys.trust(signer.kid(), &signer.public_key()).unwrap();
    keys
}

fn sign(signer: &InProcessProofSigner, proof: &ExecutionProof) -> SignedExecutionProof {
    signer.sign_now(proof).unwrap()
}

#[test]
fn a_signed_proof_verifies_and_carries_a_kid_but_no_key() {
    let signer = signer();
    let record = sign(&signer, &proof("run", "n1"));
    assert_eq!(verify_proof(&record, &trusted(&signer)), Ok(()));
    assert_eq!(record.kid, "k1");
    assert_eq!(record.algorithm, "ed25519");
    assert_eq!(record.prev_hash, None);
    let json = serde_json::to_value(&record).unwrap();
    assert!(json.get("publicKey").is_none());
    assert_eq!(json["proof"]["kind"], "sandbox_execution");
    assert_eq!(record.payload_hash, payload_hash(&record.proof).unwrap());
}

#[test]
fn every_field_of_the_proof_is_covered_by_the_signature() {
    let signer = signer();
    let keys = trusted(&signer);
    let record = sign(&signer, &proof("run", "n1"));
    let original = serde_json::to_value(&record).unwrap();
    let fields: Vec<String> = original["proof"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert!(fields.len() >= 22, "{fields:?}");
    for field in fields {
        let mut value = original.clone();
        let slot = &mut value["proof"][&field];
        *slot = match slot.clone() {
            Value::String(s) => Value::String(format!("{s}x")),
            Value::Bool(b) => Value::Bool(!b),
            Value::Number(n) => json!(n.as_u64().unwrap() + 1),
            Value::Object(mut o) => {
                // The nested records: change their first member.
                let key = o.keys().next().unwrap().clone();
                o.insert(key, json!("changed"));
                Value::Object(o)
            }
            other => panic!("{field}: {other}"),
        };
        // A value that no longer parses is rejected as well.
        if let Ok(altered) = serde_json::from_value::<SignedExecutionProof>(value) {
            assert!(
                verify_proof(&altered, &keys).is_err(),
                "an altered {field} still verifies"
            );
        }
    }
}

#[test]
fn the_kind_is_signed() {
    let signer = signer();
    let keys = trusted(&signer);
    let mut record = sign(&signer, &proof("run", "n1"));
    record.proof.kind = "approval".to_owned();
    assert_eq!(verify_proof(&record, &keys), Err(ProofError::WrongKind));
    // Even when the attacker recomputes the payload hash, the signature no longer matches.
    let mut forged = sign(&signer, &proof("run", "n2"));
    forged.proof.kind = "other".to_owned();
    forged.payload_hash = payload_hash(&forged.proof).unwrap();
    assert!(verify_proof(&forged, &keys).is_err());
    // The signer refuses to sign any other kind.
    let mut p = proof("run", "n3");
    p.kind = "approval".to_owned();
    assert_eq!(
        signer.sign_now(&p),
        Err(SignerError::Refused(SignerRefusal::InvalidProof))
    );
}

#[test]
fn only_pinned_keys_verify() {
    let signer = signer();
    let record = sign(&signer, &proof("run", "n1"));
    // No key pinned.
    assert_eq!(
        verify_proof(&record, &TrustedKeys::new()),
        Err(ProofError::UnknownKey)
    );
    // The kid is known but the key is another one: the record cannot bring its own.
    let other = InProcessProofSigner::from_seed("k1", &[9; 32]);
    assert_eq!(
        verify_proof(&record, &trusted(&other)),
        Err(ProofError::BadSignature)
    );
    // A record signed by an attacker's key under an unknown kid.
    let attacker = InProcessProofSigner::from_seed("evil", &[9; 32]);
    let forged = sign(&attacker, &proof("run", "n1"));
    assert_eq!(
        verify_proof(&forged, &trusted(&signer)),
        Err(ProofError::UnknownKey)
    );
    // A key smuggled into the record is not part of the format.
    let mut value = serde_json::to_value(&forged).unwrap();
    value["publicKey"] = json!("AAAA");
    assert_eq!(
        verify_proof_value(&value, &trusted(&attacker)),
        Err(ProofError::Malformed)
    );
}

#[test]
fn keys_rotate_and_are_revoked_for_good() {
    let old = InProcessProofSigner::from_seed("old", &[1; 32]);
    let new = InProcessProofSigner::from_seed("new", &[2; 32]);
    let a = sign(&old, &proof("run", "n1"));
    let b = sign(&new, &proof("run", "n2"));
    let mut keys = trusted(&old);
    assert_eq!(verify_proof(&b, &keys), Err(ProofError::UnknownKey));
    keys.trust(new.kid(), &new.public_key()).unwrap();
    assert_eq!(verify_proof(&a, &keys), Ok(()));
    assert_eq!(verify_proof(&b, &keys), Ok(()));
    keys.revoke("old");
    assert_eq!(verify_proof(&a, &keys), Err(ProofError::RevokedKey));
    assert_eq!(verify_proof(&b, &keys), Ok(()));
    keys.trust("old", &old.public_key()).unwrap();
    assert!(
        !keys.is_trusted("old"),
        "a revocation is not undone by trusting again"
    );
    assert_eq!(verify_proof(&a, &keys), Err(ProofError::RevokedKey));
    assert_eq!(keys.trust("", &old.public_key()), Err(KeyError::EmptyKid));
}

#[test]
fn keys_can_be_pinned_from_base64_in_either_encoding() {
    let signer = signer();
    let raw = STANDARD_B64.encode(signer.public_key());
    let mut spki = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    spki.extend_from_slice(&signer.public_key());
    let record = sign(&signer, &proof("run", "n1"));
    for encoded in [raw, STANDARD_B64.encode(spki)] {
        let mut keys = TrustedKeys::new();
        keys.trust_base64("k1", &encoded).unwrap();
        assert_eq!(verify_proof(&record, &keys), Ok(()));
    }
    let mut keys = TrustedKeys::new();
    assert_eq!(keys.trust_base64("k1", "!!"), Err(KeyError::Malformed));
    assert_eq!(keys.trust_base64("k1", "AAAA"), Err(KeyError::Malformed));
}

use base64::engine::general_purpose::STANDARD as STANDARD_B64;
use base64::Engine as _;

#[test]
fn the_chain_is_per_run_and_every_record_links_to_the_previous_one() {
    let signer = signer();
    let a1 = sign(&signer, &proof("run-a", "n1"));
    let b1 = sign(&signer, &proof("run-b", "n1"));
    let a2 = sign(&signer, &proof("run-a", "n2"));
    assert_eq!(a1.prev_hash, None);
    assert_eq!(b1.prev_hash, None, "another run starts its own chain");
    assert_eq!(a2.prev_hash.as_deref(), Some(a1.payload_hash.as_str()));
    let keys = trusted(&signer);
    assert_eq!(verify_chain(&[a1.clone(), a2.clone()], &keys), Ok(()));
    assert_eq!(verify_chain(&[], &keys), Ok(()));
    assert_eq!(
        verify_chain(&[a1, b1, a2], &keys).unwrap_err().index,
        1,
        "a record of another run is caught where it stands"
    );
}

#[test]
fn a_chain_with_a_record_moved_removed_inserted_or_repeated_does_not_verify() {
    let signer = signer();
    let keys = trusted(&signer);
    let r: Vec<_> = (1..=4)
        .map(|i| sign(&signer, &proof("run", &format!("n{i}"))))
        .collect();
    assert_eq!(verify_chain(&r, &keys), Ok(()));
    let fault = |chain: &[SignedExecutionProof]| verify_chain(chain, &keys).unwrap_err().fault;
    // Reordered.
    assert_eq!(
        fault(&[r[0].clone(), r[2].clone(), r[1].clone(), r[3].clone()]),
        ChainFault::BrokenLink
    );
    // Removed from the front, from the middle.
    assert_eq!(fault(&r[1..]), ChainFault::BrokenLink);
    assert_eq!(
        fault(&[r[0].clone(), r[2].clone(), r[3].clone()]),
        ChainFault::BrokenLink
    );
    // Repeated.
    assert_eq!(
        fault(&[r[0].clone(), r[1].clone(), r[1].clone(), r[2].clone()]),
        ChainFault::BrokenLink
    );
    // Inserted: a record signed later for the same run, put in the middle.
    let extra = sign(&signer, &proof("run", "n5"));
    assert_eq!(
        fault(&[r[0].clone(), extra.clone(), r[1].clone()]),
        ChainFault::BrokenLink
    );
    // Cut off the end: only a known head shows it.
    assert_eq!(verify_chain(&r[..3], &keys), Ok(()));
    assert_eq!(
        verify_chain_ending_at(&r[..3], &keys, &r[3].payload_hash)
            .unwrap_err()
            .fault,
        ChainFault::WrongHead
    );
    assert_eq!(
        verify_chain_ending_at(&r, &keys, &r[3].payload_hash),
        Ok(())
    );
    assert_eq!(
        verify_chain_ending_at(&[], &keys, &r[3].payload_hash)
            .unwrap_err()
            .fault,
        ChainFault::WrongHead
    );
    // A tampered link.
    let mut tampered = r.clone();
    tampered[2].prev_hash = Some(digest("elsewhere"));
    assert_eq!(
        verify_chain(&tampered, &keys).unwrap_err().fault,
        ChainFault::Record(ProofError::BadSignature)
    );
}

#[test]
fn two_records_about_the_same_collect_are_a_duplicate() {
    // A faulty signer that signs one key twice and chains both. The honest one refuses.
    use ed25519_dalek::{Signer as _, SigningKey};
    let signer = signer();
    let keys = trusted(&signer);
    let first = sign(&signer, &proof("run", "n1"));
    let mut second_proof = proof("run", "n2");
    second_proof.idempotency_key = first.proof.idempotency_key.clone();
    let payload = payload_hash(&second_proof).unwrap();
    let message = signing_message(&payload, Some(&first.payload_hash));
    let second = SignedExecutionProof {
        proof: second_proof,
        algorithm: PROOF_ALGORITHM.to_owned(),
        domain: PROOF_DOMAIN.to_owned(),
        kid: "k1".to_owned(),
        prev_hash: Some(first.payload_hash.clone()),
        payload_hash: payload,
        signature: STANDARD_B64.encode(SigningKey::from_bytes(&SEED).sign(&message).to_bytes()),
    };
    assert_eq!(verify_proof(&second, &keys), Ok(()));
    assert_eq!(
        verify_chain(&[first, second], &keys).unwrap_err().fault,
        ChainFault::DuplicateKey
    );
}

#[test]
fn a_mixed_chain_is_rejected_at_the_foreign_record() {
    let signer = signer();
    let keys = trusted(&signer);
    let sandbox = serde_json::to_value(sign(&signer, &proof("run", "n1"))).unwrap();
    let approval = json!({
        "approvalId": "a1", "runId": "run", "status": "approved", "resolvedBy": "reviewer",
        "subject": "tool:x", "decidedAt": "2026-01-01T00:00:00Z", "algorithm": "ed25519",
        "payloadHash": digest("p"), "prevHash": null, "publicKey": "AAAA", "signature": "AAAA"
    });
    assert_eq!(
        verify_chain_values(std::slice::from_ref(&sandbox), &keys),
        Ok(())
    );
    let error = verify_chain_values(&[sandbox.clone(), approval.clone()], &keys).unwrap_err();
    assert_eq!(
        error,
        ChainError {
            index: 1,
            fault: ChainFault::Record(ProofError::NotSandboxRecord)
        }
    );
    let error = verify_chain_values(&[approval.clone(), sandbox.clone()], &keys).unwrap_err();
    assert_eq!(error.index, 0);
    assert_eq!(
        verify_proof_value(&approval, &keys),
        Err(ProofError::NotSandboxRecord)
    );
    // A sandbox-shaped record of another kind.
    let mut wrong = sandbox;
    wrong["proof"]["kind"] = json!("approval");
    assert_eq!(
        verify_proof_value(&wrong, &keys),
        Err(ProofError::WrongKind)
    );
}

#[test]
fn signing_is_idempotent_by_key_and_does_not_extend_the_chain() {
    let signer = signer();
    let p = proof("run", "n1");
    let first = sign(&signer, &p);
    assert_eq!(sign(&signer, &p), first);
    assert_eq!(signer.len(), 1);
    // The same collect asked again after a restart of the host carries other times: the record
    // signed first stands.
    let mut later = p.clone();
    later.started_at += 500;
    later.finished_at += 500;
    assert_eq!(sign(&signer, &later), first);
    // The next record chains after the first, once.
    let second = sign(&signer, &proof("run", "n2"));
    assert_eq!(
        second.prev_hash.as_deref(),
        Some(first.payload_hash.as_str())
    );
    assert_eq!(sign(&signer, &p), first);
    assert_eq!(signer.len(), 2);
}

#[test]
fn another_content_under_a_signed_key_is_refused() {
    let signer = signer();
    let p = proof("run", "n1");
    sign(&signer, &p);
    let mut altered = p;
    altered.stdout_digest = digest("something else");
    assert_eq!(
        signer.sign_now(&altered),
        Err(SignerError::Refused(SignerRefusal::Conflict))
    );
    assert_eq!(signer.len(), 1);
}

#[test]
fn every_field_that_differs_is_a_conflict_under_a_signed_key() {
    let signer = signer();
    let p = proof("run", "n1");
    sign(&signer, &p);
    let mut other = p.clone();
    other.approval_payload_hash = digest("another approval");
    assert_eq!(
        signer.sign_now(&other),
        Err(SignerError::Refused(SignerRefusal::Conflict))
    );
    let mut other = p;
    other.approval.approved_by = "someone else".to_owned();
    assert_eq!(
        signer.sign_now(&other),
        Err(SignerError::Refused(SignerRefusal::Conflict))
    );
}

#[test]
fn a_malformed_proof_is_refused_and_leaves_the_chain_alone() {
    let signer = signer();
    type Mutation = (&'static str, Box<dyn Fn(&mut ExecutionProof)>);
    let mutations: Vec<Mutation> = vec![
        ("kind", Box::new(|p| p.kind = "x".into())),
        ("digest", Box::new(|p| p.stdout_digest = "abc".into())),
        (
            "uppercase digest",
            Box::new(|p| p.outputs_digest = p.outputs_digest.to_uppercase()),
        ),
        (
            "approval hash",
            Box::new(|p| p.approval_payload_hash = String::new()),
        ),
        ("times", Box::new(|p| p.finished_at = p.started_at - 1)),
        ("empty run", Box::new(|p| p.run_id = " ".into())),
        (
            "empty approver",
            Box::new(|p| p.approval.approved_by = String::new()),
        ),
        (
            "no canonical form",
            Box::new(|p| p.declared_duration_ms = 1 << 60),
        ),
    ];
    for (name, mutate) in mutations {
        let mut p = proof("run", "n1");
        mutate(&mut p);
        assert_eq!(
            signer.sign_now(&p),
            Err(SignerError::Refused(SignerRefusal::InvalidProof)),
            "{name}"
        );
    }
    assert!(signer.is_empty());
    assert_eq!(sign(&signer, &proof("run", "n1")).prev_hash, None);
}

#[test]
fn a_full_signer_is_unavailable_and_keeps_answering_what_it_signed() {
    let signer = signer().with_max_records_per_tenant(1);
    let first = sign(&signer, &proof("run", "n1"));
    assert_eq!(
        signer.sign_now(&proof("run", "n2")),
        Err(SignerError::Unavailable)
    );
    assert_eq!(sign(&signer, &proof("run", "n1")), first);
}

#[test]
fn tampering_with_the_signature_or_the_algorithm_fails() {
    let signer = signer();
    let keys = trusted(&signer);
    let record = sign(&signer, &proof("run", "n1"));
    let mut bad = record.clone();
    bad.algorithm = "rsa".into();
    assert_eq!(
        verify_proof(&bad, &keys),
        Err(ProofError::UnsupportedAlgorithm)
    );
    let mut bad = record.clone();
    bad.signature = STANDARD_B64.encode([0u8; 64]);
    assert_eq!(verify_proof(&bad, &keys), Err(ProofError::BadSignature));
    let mut bad = record.clone();
    bad.signature = "not base64!".into();
    assert_eq!(verify_proof(&bad, &keys), Err(ProofError::Malformed));
    let mut bad = record.clone();
    bad.signature = STANDARD_B64.encode([0u8; 10]);
    assert_eq!(verify_proof(&bad, &keys), Err(ProofError::Malformed));
    let mut bad = record.clone();
    bad.prev_hash = Some("short".into());
    assert_eq!(verify_proof(&bad, &keys), Err(ProofError::Malformed));
    let mut bad = record;
    bad.payload_hash = digest("other");
    assert_eq!(
        verify_proof(&bad, &keys),
        Err(ProofError::PayloadHashMismatch)
    );
}

#[test]
fn same_content_ignores_the_two_times_only() {
    let p = proof("run", "n1");
    let mut moved = p.clone();
    moved.started_at += 1;
    moved.finished_at += 9;
    assert!(p.same_content_as(&moved));
    let mut other = p.clone();
    other.truncated = true;
    assert!(!p.same_content_as(&other));
}

#[test]
fn the_debug_of_a_signer_shows_no_key() {
    let text = format!("{:?}", signer());
    assert!(text.contains("k1"));
    assert!(!text.contains("07"));
    assert!(!text.to_lowercase().contains("secret"));
}

#[tokio::test]
async fn concurrent_signing_never_forks_a_run_chain() {
    let signer = std::sync::Arc::new(signer());
    let keys = trusted(&signer);
    let mut tasks = Vec::new();
    for i in 0..32 {
        let signer = std::sync::Arc::clone(&signer);
        tasks.push(tokio::spawn(async move {
            signer.sign(&proof("run", &format!("n{i}"))).await.unwrap()
        }));
    }
    let mut records = Vec::new();
    for task in tasks {
        records.push(task.await.unwrap());
    }
    // Put the records in chain order by following the links.
    let mut ordered = Vec::new();
    let mut prev: Option<String> = None;
    while let Some(next) = records.iter().find(|r| r.prev_hash == prev) {
        prev = Some(next.payload_hash.clone());
        ordered.push(next.clone());
    }
    assert_eq!(ordered.len(), 32);
    assert_eq!(verify_chain(&ordered, &keys), Ok(()));
}

#[test]
fn a_key_of_small_order_is_not_pinned_and_would_verify_nothing_if_it_were() {
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert_eq!(
        TrustedKeys::new().trust("weak", &identity),
        Err(KeyError::WeakKey)
    );
    // Under the identity key, R = identity and S = 0 is a "signature" of every message for the
    // lenient check; the strict one that the verifier uses refuses it.
    let mut raw = [0u8; 64];
    raw[0] = 1;
    let key = VerifyingKey::from_bytes(&identity).unwrap();
    assert!(key
        .verify(b"anything", &Signature::from_bytes(&raw))
        .is_ok());
    let mut record = sign(&signer(), &proof("run", "n1"));
    record.kid = "weak".to_owned();
    record.signature = STANDARD_B64.encode(raw);
    let mut keys = TrustedKeys::new();
    keys.pin_unchecked("weak", key);
    assert_eq!(verify_proof(&record, &keys), Err(ProofError::BadSignature));
}

#[test]
fn a_spki_buffer_with_a_wrong_prefix_is_malformed() {
    let signer = signer();
    let mut spki = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    spki.extend_from_slice(&signer.public_key());
    assert_eq!(spki.len(), 44);
    spki[4] ^= 0xff;
    let mut keys = TrustedKeys::new();
    assert_eq!(
        keys.trust_base64("k1", &STANDARD_B64.encode(&spki)),
        Err(KeyError::Malformed)
    );
    assert!(!keys.is_trusted("k1"));
}

#[test]
fn a_kid_is_pinned_to_one_key_for_good() {
    let a = InProcessProofSigner::from_seed("k", &[1; 32]);
    let b = InProcessProofSigner::from_seed("k", &[2; 32]);
    let mut keys = TrustedKeys::new();
    keys.trust("k", &a.public_key()).unwrap();
    assert_eq!(
        keys.trust("k", &a.public_key()),
        Ok(()),
        "the same key again is fine"
    );
    assert_eq!(keys.trust("k", &b.public_key()), Err(KeyError::Conflict));
    let record = sign(&a, &proof("run", "n1"));
    assert_eq!(
        verify_proof(&record, &keys),
        Ok(()),
        "the first key still stands"
    );
    keys.revoke("k");
    let _ = keys.trust("k", &b.public_key());
    assert!(!keys.is_trusted("k"), "a revocation stays");
    assert_eq!(verify_proof(&record, &keys), Err(ProofError::RevokedKey));
}

#[test]
fn the_length_of_a_signer_is_still_readable_after_a_poisoned_lock() {
    let signer = std::sync::Arc::new(signer());
    sign(&signer, &proof("run", "n1"));
    let held = std::sync::Arc::clone(&signer);
    let _ = std::thread::spawn(move || {
        let _guard = held.state.lock().unwrap();
        panic!("poison the lock");
    })
    .join();
    assert_eq!(signer.len(), 1);
}

/// A record signed correctly by `SEED`'s key, chained to `prev` by hand: what a faulty signer
/// that does not keep chains apart would write.
fn chained(proof: &ExecutionProof, prev: Option<&str>) -> SignedExecutionProof {
    use ed25519_dalek::{Signer as _, SigningKey};
    let payload = payload_hash(proof).unwrap();
    let message = signing_message(&payload, prev);
    SignedExecutionProof {
        proof: proof.clone(),
        algorithm: PROOF_ALGORITHM.to_owned(),
        domain: PROOF_DOMAIN.to_owned(),
        kid: "k1".to_owned(),
        prev_hash: prev.map(str::to_owned),
        payload_hash: payload,
        signature: STANDARD_B64.encode(SigningKey::from_bytes(&SEED).sign(&message).to_bytes()),
    }
}

#[test]
fn a_record_of_another_run_or_tenant_in_the_chain_is_caught_even_when_correctly_signed() {
    let signer = signer();
    let keys = trusted(&signer);
    let first = sign(&signer, &proof("run-a", "n1"));
    let mut other_run = proof("run-b", "n1");
    other_run.idempotency_key = digest("other run");
    let mut other_tenant = proof("run-a", "n2");
    other_tenant.tenant_id = "tenant-b".to_owned();
    for (what, foreign) in [("run", other_run), ("tenant", other_tenant)] {
        let second = chained(&foreign, Some(&first.payload_hash));
        assert_eq!(
            verify_proof(&second, &keys),
            Ok(()),
            "{what}: signed and linked"
        );
        assert_eq!(
            verify_chain(&[first.clone(), second], &keys).unwrap_err(),
            ChainError {
                index: 1,
                fault: ChainFault::OtherRun
            },
            "{what}"
        );
    }
}

#[test]
fn signer_chains_are_per_tenant() {
    let signer = signer();
    let a1 = sign(&signer, &proof("run", "n1"));
    let mut b = proof("run", "n1");
    b.tenant_id = "tenant-b".to_owned();
    b.idempotency_key = digest("tenant b collect");
    let b1 = sign(&signer, &b);
    assert_eq!(a1.prev_hash, None);
    assert_eq!(
        b1.prev_hash, None,
        "the same run id of another tenant has its own chain"
    );
    let a2 = sign(&signer, &proof("run", "n2"));
    assert_eq!(a2.prev_hash.as_deref(), Some(a1.payload_hash.as_str()));
    let mut b = proof("run", "n2");
    b.tenant_id = "tenant-b".to_owned();
    b.idempotency_key = digest("tenant b collect 2");
    assert_eq!(
        sign(&signer, &b).prev_hash.as_deref(),
        Some(b1.payload_hash.as_str())
    );
}

#[test]
fn an_unknown_field_in_the_declared_exit_is_refused_in_raw_records() {
    let signer = signer();
    let keys = trusted(&signer);
    let record = sign(&signer, &proof("run", "n1"));
    let valid = serde_json::to_value(&record).unwrap();
    assert_eq!(verify_proof_value(&valid, &keys), Ok(()));
    assert_eq!(
        verify_chain_values(std::slice::from_ref(&valid), &keys),
        Ok(())
    );
    for exit in [
        json!({"kind": "code", "code": 0, "extra": 1}),
        json!({"kind": "signal", "name": "TERM", "code": 0}),
        json!({"kind": "killed", "cause": "wall_time", "name": "x"}),
        json!({"kind": "code"}),
        json!("code"),
    ] {
        let mut tampered = valid.clone();
        tampered["proof"]["declaredExit"] = exit;
        assert_eq!(
            verify_proof_value(&tampered, &keys),
            Err(ProofError::Malformed)
        );
        assert_eq!(
            verify_chain_values(&[valid.clone(), tampered], &keys)
                .unwrap_err()
                .fault,
            ChainFault::Record(ProofError::Malformed)
        );
    }
    // Other nested objects of the proof refuse unknown fields by themselves.
    for path in ["limitsEffective", "approval"] {
        let mut tampered = valid.clone();
        tampered["proof"][path]["extra"] = json!(1);
        assert_eq!(
            verify_proof_value(&tampered, &keys),
            Err(ProofError::Malformed)
        );
    }
}

/// One change of one field of a proof's content, with the name of the field.
type Change = (&'static str, fn(&mut ExecutionProof));

/// Every way of changing one field of the content of a proof.
fn content_changes() -> Vec<Change> {
    vec![
        ("kind", |p| p.kind = "other".to_owned()),
        ("runId", |p| p.run_id.push('x')),
        ("nodeId", |p| p.node_id.push('x')),
        ("tenantId", |p| p.tenant_id.push('x')),
        ("sandboxId", |p| p.sandbox_id.push('x')),
        ("idempotencyKey", |p| p.idempotency_key = digest("k")),
        ("isolationDomain", |p| p.isolation_domain.push('x')),
        ("requestDigest", |p| p.request_digest = digest("r")),
        ("profileDigest", |p| p.profile_digest = digest("p")),
        ("limitsEffective", |p| {
            p.limits_effective.cpu_millicores += 1
        }),
        ("inputsDigest", |p| p.inputs_digest = digest("i")),
        ("outputsDigest", |p| p.outputs_digest = digest("o")),
        ("stdoutDigest", |p| p.stdout_digest = digest("so")),
        ("stderrDigest", |p| p.stderr_digest = digest("se")),
        ("truncated", |p| p.truncated = !p.truncated),
        ("declaredExit", |p| {
            p.declared_exit = crate::DeclaredExit::Code { code: 1 }
        }),
        ("declaredDurationMs", |p| p.declared_duration_ms += 1),
        ("violation", |p| p.violation = !p.violation),
        ("approval", |p| p.approval.call_key.push('x')),
        ("approval", |p| p.approval.approved_by.push('x')),
        ("approvalPayloadHash", |p| {
            p.approval_payload_hash = digest("a")
        }),
    ]
}

#[test]
fn same_content_sees_every_field_but_the_two_times() {
    let base = proof("run", "n1");
    let changes = content_changes();
    // Nothing is forgotten: every field of the record but the two times has a change.
    let json = serde_json::to_value(&base).unwrap();
    let mut covered: Vec<&str> = changes.iter().map(|(name, _)| *name).collect();
    covered.sort_unstable();
    covered.dedup();
    let mut fields: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|name| *name != "startedAt" && *name != "finishedAt")
        .collect();
    fields.sort_unstable();
    assert_eq!(covered, fields);
    for (name, change) in &changes {
        let mut other = base.clone();
        change(&mut other);
        assert_ne!(other, base, "{name}: the change changes nothing");
        assert!(!base.same_content_as(&other), "{name} is ignored");
        assert!(!other.same_content_as(&base), "{name} is ignored");
    }
    // The two dates alone are not content.
    let mut dated = base.clone();
    dated.started_at += 7;
    dated.finished_at += 11;
    assert!(base.same_content_as(&dated) && dated.same_content_as(&base));
}

#[test]
fn a_signer_refuses_every_changed_field_and_accepts_changed_dates() {
    let signer = signer();
    let base = proof("run", "n1");
    let first = sign(&signer, &base);
    for (name, change) in content_changes() {
        if name == "idempotencyKey" || name == "tenantId" {
            // Another key, or the same key of another tenant, is another `collect`, not a
            // conflict.
            continue;
        }
        let mut other = base.clone();
        change(&mut other);
        // A changed `kind` is an invalid proof; any other change is a conflict. Never signed.
        assert!(
            matches!(signer.sign_now(&other), Err(SignerError::Refused(_))),
            "{name} was signed"
        );
    }
    let mut dated = base.clone();
    dated.started_at += 7;
    dated.finished_at += 11;
    assert_eq!(signer.sign_now(&dated), Ok(first));
}

#[test]
fn a_signer_keeps_exactly_its_bound_of_records_per_tenant() {
    // The default bounds are part of the contract; filling them takes too long for a unit
    // test, so the constants are pinned and the behavior at the bound is shown with small ones.
    assert_eq!(DEFAULT_MAX_RECORDS, 65_536);
    assert_eq!(DEFAULT_MAX_TENANTS, 256);
    for bound in [1_usize, 3, 8] {
        let signer = signer().with_max_records_per_tenant(bound);
        let mut last = proof("run", "n0");
        for i in 0..bound {
            last = proof(&format!("run-{i}"), "n");
            signer.sign_now(&last).expect("room for the record");
        }
        assert_eq!(signer.len(), bound);
        // The record after the last one is refused; what was signed is still served.
        assert_eq!(
            signer.sign_now(&proof("run-over", "n")),
            Err(SignerError::Unavailable)
        );
        assert!(signer.sign_now(&last).is_ok());
        assert_eq!(signer.len(), bound);
    }
}

fn of_tenant(tenant: &str, node: &str) -> ExecutionProof {
    let mut p = proof("run", node);
    p.tenant_id = tenant.to_owned();
    p.idempotency_key = digest(&format!("collect/{tenant}/{node}"));
    p
}

#[test]
fn a_noisy_tenant_at_its_quota_does_not_stop_the_others() {
    let signer = signer().with_max_records_per_tenant(2);
    let noisy = [of_tenant("noisy", "a"), of_tenant("noisy", "b")];
    let signed: Vec<_> = noisy.iter().map(|p| sign(&signer, p)).collect();
    assert_eq!(
        signer.sign_now(&of_tenant("noisy", "c")),
        Err(SignerError::Unavailable)
    );
    // The quiet tenant still signs, up to its own quota, and its chain starts fresh.
    let calm = sign(&signer, &of_tenant("calm", "a"));
    assert_eq!(calm.prev_hash, None);
    sign(&signer, &of_tenant("calm", "b"));
    assert_eq!(
        signer.sign_now(&of_tenant("calm", "c")),
        Err(SignerError::Unavailable)
    );
    // What the noisy tenant already has is still served, whatever the quota.
    for (p, record) in noisy.iter().zip(&signed) {
        assert_eq!(&sign(&signer, p), record);
    }
    // The same idempotency key under another tenant is another record, not a conflict.
    let mut other = of_tenant("calm", "a");
    other.tenant_id = "third".to_owned();
    assert!(signer.sign_now(&other).is_ok());
}

#[test]
fn the_number_of_tenants_a_signer_follows_is_bounded() {
    let signer = signer().with_max_tenants(2);
    let first = sign(&signer, &of_tenant("t1", "a"));
    sign(&signer, &of_tenant("t2", "a"));
    assert_eq!(
        signer.sign_now(&of_tenant("t3", "a")),
        Err(SignerError::Unavailable)
    );
    // A followed tenant keeps signing and keeps being served.
    sign(&signer, &of_tenant("t2", "b"));
    assert_eq!(sign(&signer, &of_tenant("t1", "a")), first);
}

fn chain_of(tenant: &str, run: &str, n: usize) -> Vec<SignedExecutionProof> {
    let signer = signer();
    (0..n)
        .map(|i| {
            let mut p = proof(run, &format!("n{i}"));
            p.tenant_id = tenant.to_owned();
            sign(&signer, &p)
        })
        .collect()
}

#[test]
fn a_valid_chain_of_another_tenant_or_run_is_refused_when_a_run_is_expected() {
    let keys = trusted(&signer());
    let chain = chain_of("tenant-b", "run-B", 2);
    // Authentic and coherent: the versions without an expected run accept it.
    assert_eq!(verify_chain(&chain, &keys), Ok(()));
    assert_eq!(verify_proof(&chain[0], &keys), Ok(()));
    // It is the chain of (tenant-b, run-B), and of nobody else.
    assert_eq!(verify_chain_for(&chain, &keys, "tenant-b", "run-B"), Ok(()));
    assert_eq!(
        verify_proof_for(&chain[1], &keys, "tenant-b", "run-B"),
        Ok(())
    );
    let head = chain[1].payload_hash.clone();
    assert_eq!(
        verify_chain_ending_at_for(&chain, &keys, "tenant-b", "run-B", &head),
        Ok(())
    );
    for (tenant, run) in [
        ("tenant-a", "run-A"),
        ("tenant-a", "run-B"),
        ("tenant-b", "run-A"),
    ] {
        assert_eq!(
            verify_chain_for(&chain, &keys, tenant, run).unwrap_err(),
            ChainError {
                index: 0,
                fault: ChainFault::OtherRun
            },
            "{tenant}/{run}"
        );
        assert_eq!(
            verify_proof_for(&chain[0], &keys, tenant, run),
            Err(ProofError::OtherRun),
            "{tenant}/{run}"
        );
        assert_eq!(
            verify_chain_ending_at_for(&chain, &keys, tenant, run, &head)
                .unwrap_err()
                .fault,
            ChainFault::OtherRun
        );
    }
    // The expected run is also checked against a later record, not only the first.
    let first = chain_of("tenant-a", "run-A", 1);
    let second = chained(
        &{
            let mut p = proof("run-A", "n9");
            p.tenant_id = "tenant-a".to_owned();
            p
        },
        Some(&first[0].payload_hash),
    );
    let mixed = [first[0].clone(), second];
    assert_eq!(verify_chain_for(&mixed, &keys, "tenant-a", "run-A"), Ok(()));
    let mut foreign = proof("run-A", "n9");
    foreign.tenant_id = "tenant-b".to_owned();
    let mixed = [
        first[0].clone(),
        chained(&foreign, Some(&first[0].payload_hash)),
    ];
    assert_eq!(
        verify_chain_for(&mixed, &keys, "tenant-a", "run-A").unwrap_err(),
        ChainError {
            index: 1,
            fault: ChainFault::OtherRun
        }
    );
    // An empty chain holds nothing of any run; a known head is what requires something.
    assert_eq!(verify_chain_for(&[], &keys, "tenant-a", "run-A"), Ok(()));
    assert_eq!(
        verify_chain_ending_at_for(&[], &keys, "tenant-a", "run-A", &head)
            .unwrap_err()
            .fault,
        ChainFault::WrongHead
    );
}

#[test]
fn the_signed_bytes_are_the_domain_then_the_chain_hash() {
    assert_eq!(PROOF_SIGN_DOMAIN, b"ailu.sandbox.proof.v1\0");
    assert_eq!(
        PROOF_DOMAIN.as_bytes(),
        &PROOF_SIGN_DOMAIN[..PROOF_SIGN_DOMAIN.len() - 1]
    );
    let message = signing_message("aa", Some("bb"));
    assert!(message.starts_with(PROOF_SIGN_DOMAIN));
    assert_eq!(
        &message[PROOF_SIGN_DOMAIN.len()..],
        chain_hash("aa", Some("bb")).as_bytes()
    );
}

#[test]
fn a_record_signed_for_another_domain_or_without_one_is_refused() {
    use ed25519_dalek::{Signer as _, SigningKey};
    let signer = signer();
    let keys = trusted(&signer);
    let record = sign(&signer, &proof("run", "n1"));
    assert_eq!(record.domain, PROOF_DOMAIN);
    assert_eq!(verify_proof(&record, &keys), Ok(()));
    // Another version of the domain, or none, is refused before anything else is judged.
    for other in ["ailu.sandbox.proof.v2", ""] {
        let mut bad = record.clone();
        bad.domain = other.to_owned();
        assert_eq!(
            verify_proof(&bad, &keys),
            Err(ProofError::UnsupportedDomain)
        );
    }
    // A record of the old format has no `domain` field at all.
    let mut value = serde_json::to_value(&record).unwrap();
    value.as_object_mut().unwrap().remove("domain");
    assert_eq!(
        verify_proof_value(&value, &keys),
        Err(ProofError::Malformed)
    );
    // The old signed bytes (the bare chain hash, as the approval attestation signs them) do
    // not verify, even under the same key and with the right label.
    let mut bare = record.clone();
    bare.signature = STANDARD_B64.encode(
        SigningKey::from_bytes(&SEED)
            .sign(chain_hash(&record.payload_hash, None).as_bytes())
            .to_bytes(),
    );
    assert_eq!(verify_proof(&bare, &keys), Err(ProofError::BadSignature));
}
