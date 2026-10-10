//! Tamper-evident attestation of approval decisions: each resolved approval is
//! hashed over a canonical view and Ed25519-signed, with records chained so neither
//! a field nor the ordering can change after the fact without breaking verification.
//!
//! Byte for byte the TypeScript implementation's (ADR 0045 D3.3): the public key is
//! SPKI DER (raw keys are still accepted when verifying), and the canonical JSON is
//! JavaScript's — numbers as `JSON.stringify` writes them, object keys in UTF-16 code
//! unit order. `tests/golden_ts_attestations.rs` proves it on records the TypeScript
//! implementation signed.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApprovalError;
use crate::types::{ApprovalRequest, ApprovalStatus};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationView {
    pub approval_id: String,
    pub run_id: String,
    pub status: String,
    pub resolved_by: String,
    pub subject: String,
    /// ADR 0051 D2 — the call the decision is about, `<name>#<sha256(canonical input)>`, when the
    /// request's subject carries one (a gated tool call filed by engine 2.7+). Omitted otherwise,
    /// so a record of a request without it hashes and verifies exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_key: Option<String>,
    pub decided_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationRecord {
    #[serde(flatten)]
    pub view: AttestationView,
    pub algorithm: String,
    pub payload_hash: String,
    pub prev_hash: Option<String>,
    /// Base64 of the SPKI DER of the Ed25519 public key that verifies `signature` —
    /// what the TypeScript implementation writes. A raw 32-byte key is accepted too.
    pub public_key: String,
    /// Base64 of the 64-byte Ed25519 signature over the chain hash.
    pub signature: String,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The SPKI DER prefix of an Ed25519 public key (RFC 8410): the 32-byte key follows.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Base64 of the SPKI DER of `key` — the TypeScript implementation's `publicKey`.
fn spki_base64(key: &VerifyingKey) -> String {
    let mut der = ED25519_SPKI_PREFIX.to_vec();
    der.extend_from_slice(&key.to_bytes());
    STANDARD.encode(der)
}

/// The 32-byte key a record's `publicKey` holds: SPKI DER, or the raw key itself.
fn public_key_bytes(encoded: &str) -> Option<[u8; 32]> {
    let bytes = STANDARD.decode(encoded).ok()?;
    let raw = match bytes.len() {
        44 if bytes[..12] == ED25519_SPKI_PREFIX => &bytes[12..],
        32 => &bytes[..],
        _ => return None,
    };
    raw.try_into().ok()
}

/// A number as JavaScript's `JSON.stringify` writes it (ECMAScript `Number::toString`):
/// the shortest digits that round-trip, positional below 1e21 and from 1e-6, exponent
/// form otherwise (`1e+21`, `1e-7`); `-0` is `0`, a non-finite value `null`. Integers go
/// through f64 as JavaScript reads them.
fn js_number(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(value) => js_number_f64(value),
        None => "null".to_owned(),
    }
}

fn js_number_f64(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` writes the shortest round-trip digits: "1.2345e-7", "1e20".
    let formatted = format!("{:e}", value.abs());
    let (mantissa, exponent) = formatted
        .split_once('e')
        .unwrap_or((formatted.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The decimal point sits after `n` digits: value = 0.d1d2…dk × 10^n.
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e_sign = if e >= 0 { "+" } else { "-" };
        let (first, rest) = digits.split_at(1);
        if rest.is_empty() {
            format!("{first}e{e_sign}{}", e.abs())
        } else {
            format!("{first}.{rest}e{e_sign}{}", e.abs())
        }
    };
    format!("{sign}{body}")
}

/// Deterministic JSON with recursively sorted object keys — JavaScript's: keys in
/// UTF-16 code unit order (what `Array.prototype.sort` compares), numbers as
/// `JSON.stringify` writes them.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            let body: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        Value::String(key.clone()),
                        canonical_json(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        Value::Number(number) => js_number(number),
        other => other.to_string(),
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex(&hasher.finalize())
}

pub fn hash_view(view: &AttestationView) -> String {
    let value = serde_json::to_value(view).unwrap_or(Value::Null);
    sha256_hex(&canonical_json(&value))
}

fn chain_hash(payload_hash: &str, prev_hash: Option<&str>) -> String {
    sha256_hex(&format!("{}:{}", prev_hash.unwrap_or(""), payload_hash))
}

fn build_view(
    request: &ApprovalRequest,
    sign_call_key: bool,
) -> Result<AttestationView, ApprovalError> {
    let status = match request.status {
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Rejected => "rejected",
        ApprovalStatus::Pending => return Err(ApprovalError::Pending(request.id.0.clone())),
    };
    let subject = match request.subject.get("description").and_then(Value::as_str) {
        Some(description) => description.to_owned(),
        None => canonical_json(&request.subject),
    };
    Ok(AttestationView {
        approval_id: request.id.0.clone(),
        run_id: request.run_id.0.clone(),
        status: status.to_owned(),
        resolved_by: request
            .resolved_by
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        subject,
        call_key: request
            .subject
            .get("callKey")
            .and_then(Value::as_str)
            .filter(|_| sign_call_key)
            .map(str::to_owned),
        decided_at: request
            .resolved_at
            .clone()
            .unwrap_or_else(|| request.created_at.clone()),
    })
}

/// Signs approval decisions with an Ed25519 key pair.
pub struct Ed25519Attestor {
    signing_key: SigningKey,
    public_key_b64: String,
    /// ADR 0051 D2 — sign the `callKey` a request's subject carries. Off by default (see
    /// [`Self::signing_call_keys`]).
    sign_call_key: bool,
}

impl Default for Ed25519Attestor {
    fn default() -> Self {
        Self::generate()
    }
}

impl Ed25519Attestor {
    /// Generate a fresh key pair from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut csprng = OsRng;
        Self::from_signing_key(SigningKey::generate(&mut csprng))
    }

    /// The key pair a 32-byte Ed25519 seed derives — the same as the TypeScript
    /// implementation's for that seed (RFC 8032 keys and signatures are deterministic).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self::from_signing_key(SigningKey::from_bytes(seed))
    }

    fn from_signing_key(signing_key: SigningKey) -> Self {
        let public_key_b64 = spki_base64(&signing_key.verifying_key());
        Ed25519Attestor {
            signing_key,
            public_key_b64,
            sign_call_key: false,
        }
    }

    /// ADR 0051 D2 — sign the call key a request's subject carries (`callKey`), so the record
    /// says « this call », not only `tool:<name>`. Off by default: a host turns it on once it
    /// keeps the record's `callKey` with the rest of it — a record stored without it no longer
    /// verifies. The TypeScript `new Ed25519Attestor(keys, { signCallKey: true })`.
    #[must_use]
    pub fn signing_call_keys(mut self) -> Self {
        self.sign_call_key = true;
        self
    }

    /// Sign a resolved approval, chaining it after `prev_hash`.
    pub fn attest(
        &self,
        request: &ApprovalRequest,
        prev_hash: Option<&str>,
    ) -> Result<AttestationRecord, ApprovalError> {
        let view = build_view(request, self.sign_call_key)?;
        let payload_hash = hash_view(&view);
        let chain = chain_hash(&payload_hash, prev_hash);
        let signature = self.signing_key.sign(chain.as_bytes());
        Ok(AttestationRecord {
            view,
            algorithm: "ed25519".to_owned(),
            payload_hash,
            prev_hash: prev_hash.map(str::to_owned),
            public_key: self.public_key_b64.clone(),
            signature: STANDARD.encode(signature.to_bytes()),
        })
    }
}

/// Verify a single record: payload hash intact and signature valid.
pub fn verify_attestation(record: &AttestationRecord) -> bool {
    if hash_view(&record.view) != record.payload_hash {
        return false;
    }
    let Some(public_array) = public_key_bytes(&record.public_key) else {
        return false;
    };
    let Ok(signature_bytes) = STANDARD.decode(&record.signature) else {
        return false;
    };
    let Ok(signature_array): Result<[u8; 64], _> = signature_bytes.try_into() else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&public_array) else {
        return false;
    };
    let signature = Signature::from_bytes(&signature_array);
    let chain = chain_hash(&record.payload_hash, record.prev_hash.as_deref());
    verifying_key.verify(chain.as_bytes(), &signature).is_ok()
}

/// Verify a full chain: every record valid and correctly linked to its predecessor.
pub fn verify_chain(records: &[AttestationRecord]) -> bool {
    let mut prev: Option<&str> = None;
    for record in records {
        if record.prev_hash.as_deref() != prev {
            return false;
        }
        if !verify_attestation(record) {
            return false;
        }
        prev = Some(&record.payload_hash);
    }
    true
}

#[cfg(test)]
mod tests {
    use ailu_graph_core::{NodeId, RunId};
    use serde_json::json;

    use super::*;
    use crate::types::{ApprovalId, ApprovalRequest, ApprovalStatus};

    fn resolved(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId(id.to_owned()),
            run_id: RunId::from("run-1"),
            node_id: NodeId::from("assistant"),
            requested_by: "assistant".to_owned(),
            subject: json!({ "description": "tool:refund" }),
            status: ApprovalStatus::Approved,
            resolved_by: Some("alice".to_owned()),
            resolved_at: Some("1000".to_owned()),
            rejection_reason: None,
            created_at: "900".to_owned(),
        }
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let value = json!({ "b": 1, "a": { "y": 1, "x": 2 } });
        assert_eq!(canonical_json(&value), "{\"a\":{\"x\":2,\"y\":1},\"b\":1}");
    }

    #[test]
    fn canonical_json_writes_numbers_as_javascript_does() {
        let cases = [
            (json!(1e20), "100000000000000000000"),
            (json!(1e21), "1e+21"),
            (json!(0.1), "0.1"),
            (json!(1e-7), "1e-7"),
            (json!(0.000001), "0.000001"),
            (json!(-0.0), "0"),
            (json!(-3.25e-10), "-3.25e-10"),
            (json!(1.0 / 3.0), "0.3333333333333333"),
            (json!(123.456), "123.456"),
            (json!(42), "42"),
            (json!(-7), "-7"),
            (json!(5e-324), "5e-324"),
            (json!(1.7976931348623157e308), "1.7976931348623157e+308"),
        ];
        for (value, expected) in cases {
            assert_eq!(canonical_json(&value), expected, "{value}");
        }
    }

    #[test]
    fn canonical_json_orders_keys_by_utf16_code_units() {
        // UTF-8 byte order would put U+E000 before the astral key; JavaScript puts it after.
        let value = json!({ "\u{E000}": 1, "😀": 2, "z": 3, "é": 4, "A": 5 });
        assert_eq!(
            canonical_json(&value),
            "{\"A\":5,\"z\":3,\"é\":4,\"😀\":2,\"\u{E000}\":1}"
        );
    }

    #[test]
    fn writes_the_spki_public_key_and_still_reads_a_raw_one() {
        let attestor = Ed25519Attestor::from_seed(&[7; 32]);
        let record = attestor.attest(&resolved("approval-1"), None).unwrap();
        assert!(record.public_key.starts_with("MCowBQYDK2VwAyEA"));
        assert!(verify_attestation(&record));

        let spki = STANDARD.decode(&record.public_key).unwrap();
        let mut raw = record.clone();
        raw.public_key = STANDARD.encode(&spki[12..]);
        assert!(verify_attestation(&raw));

        let mut truncated = record.clone();
        truncated.public_key = STANDARD.encode(&spki[..40]);
        assert!(!verify_attestation(&truncated));
    }

    #[test]
    fn signs_and_verifies_a_decision() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("approval-1"), None).unwrap();
        assert_eq!(record.algorithm, "ed25519");
        assert_eq!(record.view.subject, "tool:refund");
        assert!(verify_attestation(&record));
    }

    #[test]
    fn signs_the_call_key_a_request_carries_when_asked_and_only_then() {
        let key = format!("refund#{}", "a".repeat(64));
        let mut request = resolved("approval-1");
        request.subject =
            json!({ "description": "tool:refund", "callKey": key, "input": { "amount": 40 } });
        // By default the record is the one an earlier attestor wrote: no call key.
        let default = Ed25519Attestor::generate().attest(&request, None).unwrap();
        assert_eq!(default.view.call_key, None);

        let attestor = Ed25519Attestor::generate().signing_call_keys();
        let record = attestor.attest(&request, None).unwrap();
        assert_eq!(record.view.subject, "tool:refund");
        assert_eq!(record.view.call_key.as_deref(), Some(key.as_str()));
        assert!(verify_attestation(&record));

        // The call is signed: another call, or none, breaks the record.
        let mut other = record.clone();
        other.view.call_key = Some(format!("refund#{}", "b".repeat(64)));
        assert!(!verify_attestation(&other));
        let mut dropped = record.clone();
        dropped.view.call_key = None;
        assert!(!verify_attestation(&dropped));

        // A request without one is attested as before: no `callKey` on the wire.
        let plain = attestor.attest(&resolved("approval-2"), None).unwrap();
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("callKey").is_none());
    }

    #[test]
    fn detects_tampering_of_any_field() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("approval-1"), None).unwrap();

        let mut tampered = record.clone();
        tampered.view.resolved_by = "mallory".to_owned();
        assert!(!verify_attestation(&tampered));

        let mut rehashed = record.clone();
        rehashed.payload_hash = "deadbeef".to_owned();
        assert!(!verify_attestation(&rehashed));
    }

    #[test]
    fn chains_records_and_rejects_reordering() {
        let attestor = Ed25519Attestor::generate();
        let first = attestor.attest(&resolved("approval-1"), None).unwrap();
        let second = attestor
            .attest(&resolved("approval-2"), Some(&first.payload_hash))
            .unwrap();

        assert_eq!(
            second.prev_hash.as_deref(),
            Some(first.payload_hash.as_str())
        );
        assert!(verify_chain(&[first.clone(), second.clone()]));
        assert!(!verify_chain(&[second, first]));
    }

    #[test]
    fn refuses_to_attest_a_pending_approval() {
        let attestor = Ed25519Attestor::generate();
        let mut pending = resolved("approval-1");
        pending.status = ApprovalStatus::Pending;
        pending.resolved_by = None;
        assert!(matches!(
            attestor.attest(&pending, None),
            Err(ApprovalError::Pending(_))
        ));
    }
}
