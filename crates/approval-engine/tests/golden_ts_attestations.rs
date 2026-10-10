//! ADR 0045 D3.3 — the Rust attestation is the TypeScript one, byte for byte.
//!
//! `fixtures/ts-attestations.json` holds approval decisions signed by the TypeScript
//! implementation (`packages/approval-engine/src/attestation.ts`) with a fixed Ed25519
//! seed, over inputs chosen where two canonical JSONs could disagree: numbers JavaScript
//! writes positionally or with an exponent, keys outside the BMP, escapes. Regenerate it
//! with `scripts/golden/attestations.ts`.

use ailu_approval_engine::{verify_chain, ApprovalRequest, AttestationRecord, Ed25519Attestor};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    seed_hex: String,
    requests: Vec<ApprovalRequest>,
    records: Vec<AttestationRecord>,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/ts-attestations.json")).expect("fixture parses")
}

fn seed(hex: &str) -> [u8; 32] {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex seed"))
        .collect();
    bytes.try_into().expect("32-byte seed")
}

#[test]
fn verifies_the_chain_the_typescript_implementation_signed() {
    let fixture = fixture();
    assert_eq!(fixture.records.len(), 5);
    assert!(verify_chain(&fixture.records));
}

#[test]
fn signs_the_same_decisions_into_the_same_bytes() {
    let fixture = fixture();
    // The fixture is signed with `signCallKey: true` (ADR 0051 D2): only a request whose subject
    // carries a call key differs from a default attestor's record.
    let attestor = Ed25519Attestor::from_seed(&seed(&fixture.seed_hex)).signing_call_keys();
    let mut prev: Option<String> = None;
    for (request, expected) in fixture.requests.iter().zip(&fixture.records) {
        let record = attestor
            .attest(request, prev.as_deref())
            .expect("a resolved decision is attested");
        assert_eq!(&record, expected, "decision {}", request.id.0);
        prev = Some(record.payload_hash.clone());
    }
}

#[test]
fn a_changed_field_no_longer_verifies() {
    let mut records = fixture().records;
    records[1].view.resolved_by = "mallory".to_owned();
    assert!(!verify_chain(&records));
}
