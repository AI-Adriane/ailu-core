//! Tamper-evident attestation of approval decisions: each resolved approval is
//! hashed over a canonical view and Ed25519-signed, with records chained so neither
//! a field nor the ordering can change after the fact without breaking verification.

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
    /// Base64 of the 32-byte Ed25519 public key that verifies `signature`.
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

/// Deterministic JSON with recursively sorted object keys.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
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

fn build_view(request: &ApprovalRequest) -> Result<AttestationView, ApprovalError> {
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
        let signing_key = SigningKey::generate(&mut csprng);
        let public_key_b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());
        Ed25519Attestor {
            signing_key,
            public_key_b64,
        }
    }

    /// Sign a resolved approval, chaining it after `prev_hash`.
    pub fn attest(
        &self,
        request: &ApprovalRequest,
        prev_hash: Option<&str>,
    ) -> Result<AttestationRecord, ApprovalError> {
        let view = build_view(request)?;
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
    let Ok(public_bytes) = STANDARD.decode(&record.public_key) else {
        return false;
    };
    let Ok(signature_bytes) = STANDARD.decode(&record.signature) else {
        return false;
    };
    let Ok(public_array): Result<[u8; 32], _> = public_bytes.try_into() else {
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
    fn signs_and_verifies_a_decision() {
        let attestor = Ed25519Attestor::generate();
        let record = attestor.attest(&resolved("approval-1"), None).unwrap();
        assert_eq!(record.algorithm, "ed25519");
        assert_eq!(record.view.subject, "tool:refund");
        assert!(verify_attestation(&record));
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
