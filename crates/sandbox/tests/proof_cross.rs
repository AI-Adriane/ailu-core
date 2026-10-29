//! The sandbox chain and the approval chain use the same primitives and never accept each
//! other's records. These tests run both implementations side by side.

use ailu_approval_engine::{
    canonical_json as approval_canonical_json, verify_attestation,
    verify_chain as verify_approvals, ApprovalRequest, ApprovalStatus, AttestationRecord,
    Ed25519Attestor,
};
use ailu_graph_core::{NodeId, RunId};
use ailu_sandbox::proof::{
    canonical_json, chain_hash, verify_chain_values, verify_proof_value, ChainFault, ProofError,
};
use ailu_sandbox::{
    ExecutionProof, InProcessProofSigner, ProofSigner, SignedExecutionProof, TrustedKeys,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{json, Value};

fn sample() -> ExecutionProof {
    let vectors: Value = serde_json::from_str(include_str!("vectors/sandbox_proof.json")).unwrap();
    serde_json::from_value(vectors["signedProof"]["records"][0]["proof"].clone()).unwrap()
}

fn approval(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        id: ailu_approval_engine::ApprovalId(id.to_owned()),
        run_id: RunId::from("run-1"),
        node_id: NodeId::from("node-1"),
        requested_by: "agent".to_owned(),
        subject: json!({ "description": "tool:refund" }),
        status: ApprovalStatus::Approved,
        resolved_by: Some("reviewer".to_owned()),
        resolved_at: Some("1000".to_owned()),
        rejection_reason: None,
        created_at: "900".to_owned(),
    }
}

#[test]
fn both_canonical_forms_write_the_same_bytes_for_what_a_proof_holds() {
    let values = [
        serde_json::to_value(sample()).unwrap(),
        json!({"b": [1, 2, {"z": null, "y": true}], "a": "x", "é": "\u{1F600}\u{E000}", "A": 0}),
        json!({"\u{E000}": 1, "😀": 2, "z": 3, "é": 4, "A": 5}),
        json!({"quote": "a\"b\\c\n\t\u{1}\u{7f}\u{2028}", "n": 9_007_199_254_740_991u64, "m": -42}),
        json!([]),
        json!({}),
    ];
    for value in values {
        assert_eq!(
            canonical_json(&value).unwrap(),
            approval_canonical_json(&value),
            "{value}"
        );
    }
}

#[test]
fn an_approval_signature_is_over_the_chain_hash_the_sandbox_chain_computes() {
    // Same chaining rule: the signature covers sha256("<prev>:<payload>").
    let attestor = Ed25519Attestor::from_seed(&[3; 32]);
    let first = attestor.attest(&approval("a1"), None).unwrap();
    let second = attestor
        .attest(&approval("a2"), Some(&first.payload_hash))
        .unwrap();
    for record in [&first, &second] {
        let spki = STANDARD.decode(&record.public_key).unwrap();
        let key = VerifyingKey::from_bytes(spki[12..].try_into().unwrap()).unwrap();
        let signature: [u8; 64] = STANDARD
            .decode(&record.signature)
            .unwrap()
            .try_into()
            .unwrap();
        let chain = chain_hash(&record.payload_hash, record.prev_hash.as_deref());
        key.verify_strict(chain.as_bytes(), &Signature::from_bytes(&signature))
            .unwrap();
    }
}

async fn sandbox_chain() -> (Vec<SignedExecutionProof>, TrustedKeys) {
    let signer = InProcessProofSigner::from_seed("k1", &[7; 32]);
    let mut keys = TrustedKeys::new();
    keys.trust("k1", &signer.public_key()).unwrap();
    let mut records = Vec::new();
    for node in ["n1", "n2"] {
        let mut proof = sample();
        proof.node_id = node.to_owned();
        proof.idempotency_key = format!("{:0>64}", node);
        records.push(signer.sign(&proof).await.unwrap());
    }
    (records, keys)
}

#[tokio::test]
async fn the_approval_verifier_rejects_a_sandbox_chain() {
    let (records, _) = sandbox_chain().await;
    for record in &records {
        let value = serde_json::to_value(record).unwrap();
        // It does not even parse as an approval record.
        assert!(serde_json::from_value::<AttestationRecord>(value.clone()).is_err());
    }
    // A forgery that dresses a sandbox record's hashes and signature as an approval record
    // does not verify either: the hash of the approval view is not the sandbox payload hash.
    let attestor = Ed25519Attestor::from_seed(&[7; 32]);
    let mut dressed = attestor.attest(&approval("a1"), None).unwrap();
    dressed.payload_hash = records[0].payload_hash.clone();
    dressed.signature = records[0].signature.clone();
    assert!(!verify_attestation(&dressed));
    assert!(!verify_approvals(&[dressed]));
}

#[tokio::test]
async fn the_sandbox_verifier_rejects_an_approval_chain_and_a_mixed_one() {
    let (records, keys) = sandbox_chain().await;
    let attestor = Ed25519Attestor::from_seed(&[7; 32]);
    let first = attestor.attest(&approval("a1"), None).unwrap();
    let second = attestor
        .attest(&approval("a2"), Some(&first.payload_hash))
        .unwrap();
    assert!(verify_approvals(&[first.clone(), second.clone()]));
    let approvals: Vec<Value> = [first, second]
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    for value in &approvals {
        assert_eq!(
            verify_proof_value(value, &keys),
            Err(ProofError::NotSandboxRecord)
        );
    }
    let sandbox: Vec<Value> = records
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    assert_eq!(verify_chain_values(&sandbox, &keys), Ok(()));
    assert_eq!(
        verify_chain_values(&approvals, &keys).unwrap_err().fault,
        ChainFault::Record(ProofError::NotSandboxRecord)
    );
    for mixed in [
        vec![sandbox[0].clone(), approvals[0].clone(), sandbox[1].clone()],
        vec![approvals[0].clone(), sandbox[0].clone()],
        vec![sandbox[0].clone(), sandbox[1].clone(), approvals[1].clone()],
    ] {
        assert!(verify_chain_values(&mixed, &keys).is_err());
    }
}
