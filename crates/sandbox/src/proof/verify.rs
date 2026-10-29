//! Verifying signed proofs and sandbox chains against keys the verifier pinned.
//!
//! A record names the key that signed it (`kid`) and carries no key. The verifier looks the
//! `kid` up in its own [`TrustedKeys`], so an attacker who can write a record cannot bring the
//! key that verifies it. Rotation is several `kid` trusted at once; revocation is
//! [`TrustedKeys::revoke`], which wins over any later `trust`.

use super::{
    canon::{chain_hash, payload_hash},
    SignedExecutionProof, PROOF_ALGORITHM, PROOF_KIND,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The SPKI DER prefix of an Ed25519 public key (RFC 8410): the 32-byte key follows.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// A key could not be pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The `kid` is empty.
    #[error("the key identifier is empty")]
    EmptyKid,
    /// The bytes are not an Ed25519 public key (raw, or SPKI DER in base64).
    #[error("not an Ed25519 public key")]
    Malformed,
}

/// The keys a verifier trusts, by `kid`. Pinned by the verifier; never taken from a record.
#[derive(Clone, Debug, Default)]
pub struct TrustedKeys {
    keys: HashMap<String, VerifyingKey>,
    revoked: HashSet<String>,
}

impl TrustedKeys {
    /// No key: nothing verifies.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust the raw 32-byte Ed25519 public key under `kid`. Trusting several `kid` at once is
    /// how a key is rotated: the new one is added before the old one is revoked.
    pub fn trust(&mut self, kid: impl Into<String>, public_key: &[u8; 32]) -> Result<(), KeyError> {
        let kid = kid.into();
        if kid.is_empty() {
            return Err(KeyError::EmptyKid);
        }
        let key = VerifyingKey::from_bytes(public_key).map_err(|_| KeyError::Malformed)?;
        self.keys.insert(kid, key);
        Ok(())
    }

    /// Like [`TrustedKeys::trust`], from the base64 of a raw key or of its SPKI DER.
    pub fn trust_base64(&mut self, kid: impl Into<String>, encoded: &str) -> Result<(), KeyError> {
        let bytes = STANDARD.decode(encoded).map_err(|_| KeyError::Malformed)?;
        let raw = match bytes.len() {
            44 if bytes[..12] == ED25519_SPKI_PREFIX => &bytes[12..],
            32 => &bytes[..],
            _ => return Err(KeyError::Malformed),
        };
        let raw: [u8; 32] = raw.try_into().map_err(|_| KeyError::Malformed)?;
        self.trust(kid, &raw)
    }

    /// Stop trusting `kid`, for good: records it signed no longer verify, and a later `trust`
    /// of the same `kid` does not bring it back.
    pub fn revoke(&mut self, kid: &str) {
        self.keys.remove(kid);
        self.revoked.insert(kid.to_owned());
    }

    /// Whether `kid` is pinned and not revoked.
    pub fn is_trusted(&self, kid: &str) -> bool {
        !self.revoked.contains(kid) && self.keys.contains_key(kid)
    }
}

/// Why a record does not verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The value is not a sandbox proof record at all (an approval attestation, say).
    #[error("not a sandbox proof record")]
    NotSandboxRecord,
    /// The record has the shape of a sandbox proof but is malformed (an unknown field such as
    /// an embedded key, a value that has no canonical form, a signature that is not 64 bytes).
    #[error("malformed record")]
    Malformed,
    /// `kind` is not `sandbox_execution`.
    #[error("wrong record kind")]
    WrongKind,
    /// The algorithm is not Ed25519.
    #[error("unsupported algorithm")]
    UnsupportedAlgorithm,
    /// The `kid` is not one the verifier pinned.
    #[error("unknown key")]
    UnknownKey,
    /// The `kid` was revoked.
    #[error("revoked key")]
    RevokedKey,
    /// The payload hash is not the hash of the proof: a field was altered.
    #[error("payload hash mismatch")]
    PayloadHashMismatch,
    /// The signature does not verify under the pinned key.
    #[error("bad signature")]
    BadSignature,
}

/// Why a chain does not verify, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("record {index}: {fault}")]
pub struct ChainError {
    /// Position, in the order given, of the record at fault.
    pub index: usize,
    pub fault: ChainFault,
}

/// What is wrong with a record of a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChainFault {
    /// The record itself does not verify.
    #[error("{0}")]
    Record(ProofError),
    /// `prev_hash` is not the previous record's payload hash (or not `None` for the first):
    /// a record was removed, inserted, repeated or moved.
    #[error("broken link")]
    BrokenLink,
    /// The record belongs to another tenant or run than the first: one chain is one run's.
    #[error("record of another run")]
    OtherRun,
    /// Two records of the chain are about the same `collect`.
    #[error("duplicate idempotency key")]
    DuplicateKey,
    /// The chain does not end at the head the verifier knows: records were cut off its end.
    #[error("chain does not end at the expected head")]
    WrongHead,
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Verify one record: sandbox `kind`, Ed25519, a `kid` the verifier pinned and did not revoke,
/// the payload hash of the proof, and the signature over the chain hash.
///
/// The key is looked up in `trusted` by `kid`. Nothing in the record can supply or replace it.
pub fn verify_proof(
    record: &SignedExecutionProof,
    trusted: &TrustedKeys,
) -> Result<(), ProofError> {
    if record.proof.kind != PROOF_KIND {
        return Err(ProofError::WrongKind);
    }
    if record.algorithm != PROOF_ALGORITHM {
        return Err(ProofError::UnsupportedAlgorithm);
    }
    if trusted.revoked.contains(&record.kid) {
        return Err(ProofError::RevokedKey);
    }
    let key = trusted
        .keys
        .get(&record.kid)
        .ok_or(ProofError::UnknownKey)?;
    if record.prev_hash.as_deref().is_some_and(|p| !is_hex64(p)) {
        return Err(ProofError::Malformed);
    }
    let computed = payload_hash(&record.proof).ok_or(ProofError::Malformed)?;
    if computed != record.payload_hash {
        return Err(ProofError::PayloadHashMismatch);
    }
    let bytes = STANDARD
        .decode(&record.signature)
        .map_err(|_| ProofError::Malformed)?;
    let bytes: [u8; 64] = bytes.try_into().map_err(|_| ProofError::Malformed)?;
    let chain = chain_hash(&record.payload_hash, record.prev_hash.as_deref());
    key.verify_strict(chain.as_bytes(), &Signature::from_bytes(&bytes))
        .map_err(|_| ProofError::BadSignature)
}

/// Verify a value that is meant to be a sandbox record. A value that is not one (it has no
/// `proof` object, or its `kind` is not `sandbox_execution`, as an approval attestation never
/// is) is [`ProofError::NotSandboxRecord`] or [`ProofError::WrongKind`]; a record with an
/// unknown field (an embedded key, say) is [`ProofError::Malformed`].
pub fn verify_proof_value(value: &Value, trusted: &TrustedKeys) -> Result<(), ProofError> {
    let kind = value
        .get("proof")
        .and_then(|proof| proof.get("kind"))
        .ok_or(ProofError::NotSandboxRecord)?;
    if kind.as_str() != Some(PROOF_KIND) {
        return Err(ProofError::WrongKind);
    }
    let record: SignedExecutionProof =
        serde_json::from_value(value.clone()).map_err(|_| ProofError::Malformed)?;
    verify_proof(&record, trusted)
}

/// Verify a run's sandbox chain, records in chain order.
///
/// Every record must verify; the first has no `prev_hash` and each next one has the previous
/// record's `payload_hash`; all belong to one tenant and run; and no two are about the same
/// `collect`. A record removed from the front or the middle, inserted, repeated or moved is
/// caught. Records cut off the **end** are caught only against a head the verifier knows:
/// see [`verify_chain_ending_at`]. An empty chain verifies.
pub fn verify_chain(
    records: &[SignedExecutionProof],
    trusted: &TrustedKeys,
) -> Result<(), ChainError> {
    let mut prev: Option<&str> = None;
    let mut keys = HashSet::new();
    for (index, record) in records.iter().enumerate() {
        let at = |fault| ChainError { index, fault };
        verify_proof(record, trusted).map_err(|e| at(ChainFault::Record(e)))?;
        if record.prev_hash.as_deref() != prev {
            return Err(at(ChainFault::BrokenLink));
        }
        if let Some(first) = records.first() {
            if record.proof.tenant_id != first.proof.tenant_id
                || record.proof.run_id != first.proof.run_id
            {
                return Err(at(ChainFault::OtherRun));
            }
        }
        if !keys.insert(record.proof.idempotency_key.as_str()) {
            return Err(at(ChainFault::DuplicateKey));
        }
        prev = Some(&record.payload_hash);
    }
    Ok(())
}

/// Like [`verify_chain`], and the last record must be the one whose `payload_hash` is `head`
/// (the head the verifier recorded): a chain with records cut off its end fails.
pub fn verify_chain_ending_at(
    records: &[SignedExecutionProof],
    trusted: &TrustedKeys,
    head: &str,
) -> Result<(), ChainError> {
    verify_chain(records, trusted)?;
    match records.last() {
        Some(last) if last.payload_hash == head => Ok(()),
        _ => Err(ChainError {
            index: records.len().saturating_sub(1),
            fault: ChainFault::WrongHead,
        }),
    }
}

/// Verify a chain given as raw values. A chain that mixes sandbox proofs with other records
/// (approval attestations) is rejected at the first one that is not a sandbox proof: the two
/// chains are separate, linked only by `approval_payload_hash`.
pub fn verify_chain_values(values: &[Value], trusted: &TrustedKeys) -> Result<(), ChainError> {
    let mut records = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        // Parse the record by the checks of `verify_proof_value`, so that the position of a
        // record that is not a sandbox proof is the one reported.
        let kind_ok = value
            .get("proof")
            .and_then(|proof| proof.get("kind"))
            .ok_or(ProofError::NotSandboxRecord)
            .and_then(|kind| {
                if kind.as_str() == Some(PROOF_KIND) {
                    Ok(())
                } else {
                    Err(ProofError::WrongKind)
                }
            });
        let record = kind_ok.and_then(|()| {
            serde_json::from_value::<SignedExecutionProof>(value.clone())
                .map_err(|_| ProofError::Malformed)
        });
        match record {
            Ok(record) => records.push(record),
            Err(e) => {
                return Err(ChainError {
                    index,
                    fault: ChainFault::Record(e),
                })
            }
        }
    }
    verify_chain(&records, trusted)
}
