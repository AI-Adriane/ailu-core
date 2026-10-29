//! The shared vectors (`vectors/sandbox_proof.json`): the recipe of the call key, and signed
//! records made with a fixed test key. They were produced by an independent implementation of
//! the recipe, so that a second implementation (in another language) can check itself against
//! the same file. The key in the file is a **test key**; it protects nothing.

use ailu_graph_core::{NodeId, RunId};
use ailu_sandbox::proof::{canonical_proof_json, payload_hash, verify_chain, verify_chain_values};
use ailu_sandbox::{
    ApprovalRequest, CreateRequest, ExecRequest, InProcessProofSigner, ProofSigner, SandboxContext,
    SignedExecutionProof, TenantId, TrustedKeys,
};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("vectors/sandbox_proof.json")).expect("valid vectors")
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_else(|| panic!("{field}"))
}

#[test]
fn the_call_key_recipe_gives_the_published_keys() {
    let vectors = vectors();
    let cases = vectors["callKey"]["cases"].as_array().unwrap();
    assert!(cases.len() >= 4);
    for case in cases {
        let ctx = SandboxContext {
            tenant_id: TenantId::new(text(&case["context"], "tenant")),
            run_id: RunId::from(text(&case["context"], "run")),
            node_id: NodeId::from(text(&case["context"], "node")),
            attempt: 1,
        };
        let create: CreateRequest = serde_json::from_value(case["create"].clone()).unwrap();
        let exec: ExecRequest = serde_json::from_value(case["exec"].clone()).unwrap();
        let request = ApprovalRequest::for_call(&ctx, &create, &exec);
        assert_eq!(
            request.call_key,
            text(case, "callKey"),
            "{}",
            text(case, "name")
        );
    }
}

#[test]
fn the_call_key_ignores_the_attempt_and_depends_on_everything_else() {
    let vectors = vectors();
    let case = &vectors["callKey"]["cases"][1];
    let ctx = |attempt| SandboxContext {
        tenant_id: TenantId::new("tenant-a"),
        run_id: RunId::from("run-1"),
        node_id: NodeId::from("node-2"),
        attempt,
    };
    let create: CreateRequest = serde_json::from_value(case["create"].clone()).unwrap();
    let exec: ExecRequest = serde_json::from_value(case["exec"].clone()).unwrap();
    let key = |ctx: &SandboxContext, exec: &ExecRequest| {
        ApprovalRequest::for_call(ctx, &create, exec).call_key
    };
    assert_eq!(key(&ctx(1), &exec), key(&ctx(9), &exec));
    let mut other = exec.clone();
    other.stdin = Some(vec![104, 105, 0, 254]);
    assert_ne!(key(&ctx(1), &exec), key(&ctx(1), &other));
}

fn records(vectors: &Value) -> Vec<SignedExecutionProof> {
    vectors["signedProof"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| serde_json::from_value(record.clone()).unwrap())
        .collect()
}

fn pinned(vectors: &Value) -> TrustedKeys {
    let signed = &vectors["signedProof"];
    let key: [u8; 32] = hex_bytes(text(signed, "publicKeyHex")).try_into().unwrap();
    let mut keys = TrustedKeys::new();
    keys.trust(text(signed, "kid"), &key).unwrap();
    keys
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn the_published_records_verify_against_the_pinned_test_key() {
    let vectors = vectors();
    let records = records(&vectors);
    assert_eq!(records.len(), 2);
    let keys = pinned(&vectors);
    assert_eq!(verify_chain(&records, &keys), Ok(()));
    let values = vectors["signedProof"]["records"].as_array().unwrap();
    assert_eq!(verify_chain_values(values, &keys), Ok(()));
    assert_eq!(records[0].prev_hash, None);
    assert_eq!(
        records[1].prev_hash.as_deref(),
        Some(records[0].payload_hash.as_str())
    );
    // Reversed, or without the first, the published chain does not verify.
    assert!(verify_chain(&[records[1].clone(), records[0].clone()], &keys).is_err());
    assert!(verify_chain(&records[1..], &keys).is_err());
    // Under no key, nothing verifies.
    assert!(verify_chain(&records, &TrustedKeys::new()).is_err());
}

#[test]
fn the_canonical_json_and_hashes_are_those_of_the_vectors() {
    let vectors = vectors();
    let texts = vectors["signedProof"]["canonicalJson"].as_array().unwrap();
    for (record, expected) in records(&vectors).iter().zip(texts) {
        assert_eq!(
            canonical_proof_json(&record.proof).unwrap(),
            expected.as_str().unwrap()
        );
        assert_eq!(payload_hash(&record.proof).unwrap(), record.payload_hash);
    }
}

#[tokio::test]
async fn the_in_process_signer_reproduces_the_published_records_byte_for_byte() {
    let vectors = vectors();
    let signed = &vectors["signedProof"];
    let seed: [u8; 32] = hex_bytes(text(signed, "seedHex")).try_into().unwrap();
    let signer = InProcessProofSigner::from_seed(text(signed, "kid"), &seed);
    assert_eq!(
        signer.public_key().to_vec(),
        hex_bytes(text(signed, "publicKeyHex"))
    );
    for expected in records(&vectors) {
        // Ed25519 is deterministic (RFC 8032), so the same key, proof and previous hash give the
        // same signature.
        assert_eq!(signer.sign(&expected.proof).await.unwrap(), expected);
    }
}
