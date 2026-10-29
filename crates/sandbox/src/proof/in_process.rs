//! A proof signer that holds its key in the host's process.
//!
//! **The signing key lives in the same process as the engine and the guard.** Anyone who can
//! read that process's memory, or run code in it, can sign any proof, so a proof from this
//! signer shows only that the host's own process vouched for it. It is meant for tests and for
//! self-hosting where the host is trusted with the key. A deployment that wants the key out of
//! reach of the host implements [`ProofSigner`] over a key service of its own.
//!
//! This signer is for tests and self-hosting only: the key is in the process.
//!
//! The chain, the records and the idempotency table are in memory: they are lost when the
//! process ends, and a restarted signer starts a new chain. A new key is a new chain too: the
//! first record under it has no `prev_hash`, so a verifier that joins the records of two keys
//! (or of two lives of the signer) into one chain gets `BrokenLink`, which is visible, not
//! silent. Verify each chain apart.
//!
//! Memory is bounded **per tenant**, so that one noisy tenant cannot make the signer refuse the
//! others: each tenant keeps at most [`DEFAULT_MAX_RECORDS_PER_TENANT`] (4,096) records
//! ([`InProcessProofSigner::with_max_records_per_tenant`]), and the signer follows at most
//! [`DEFAULT_MAX_TENANTS`] (16) tenants ([`InProcessProofSigner::with_max_tenants`]). Beyond
//! the first bound a tenant's new records get `Unavailable`; beyond the second, the **new**
//! tenants do. A key already signed is always served again. A signed record is about 1.5 KB of
//! JSON, so the worst case under the defaults is 16 x 4,096 x 1.5 KB, about 100 MB. These are
//! the bounds of the in-process signer, not a production setting: a host that needs more
//! raises them knowingly, or uses a signer backed by a service of its own.
//!
//! The seed the caller supplies is the caller's: this crate copies it into the signing key and
//! does not erase the caller's array. The signing key is erased when the signer is dropped.

use super::canon::{payload_hash, signing_message};
use super::{
    ExecutionProof, ProofSigner, SignedExecutionProof, SignerError, SignerRefusal, PROOF_ALGORITHM,
    PROOF_DOMAIN, PROOF_KIND,
};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use std::collections::HashMap;
use std::sync::Mutex;

/// How many records an in-process signer keeps by default, per tenant.
pub const DEFAULT_MAX_RECORDS_PER_TENANT: usize = 4_096;

/// How many tenants an in-process signer follows by default.
pub const DEFAULT_MAX_TENANTS: usize = 16;

#[derive(Default)]
pub(super) struct State {
    /// Every record signed, by tenant and idempotency key.
    by_key: HashMap<(String, String), SignedExecutionProof>,
    /// How many records each tenant holds.
    per_tenant: HashMap<String, usize>,
    /// The `payload_hash` of the last record of each `(tenant, run)` chain.
    tails: HashMap<(String, String), String>,
}

/// An Ed25519 [`ProofSigner`] with its key in this process, one chain per `(tenant, run)`,
/// idempotent by `idempotency_key`. Read the module documentation before relying on it.
///
/// It checks the shape of every proof before signing it ([`SignerRefusal::InvalidProof`]) and
/// signs under one lock, so concurrent calls for a run never fork its chain.
pub struct InProcessProofSigner {
    kid: String,
    key: SigningKey,
    max_records: usize,
    max_tenants: usize,
    pub(super) state: Mutex<State>,
}

impl std::fmt::Debug for InProcessProofSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessProofSigner")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl InProcessProofSigner {
    /// A signer for the key `seed` derives (RFC 8032), named `kid` in its records. The host
    /// supplies the seed from its own secret store; this crate generates no key.
    pub fn from_seed(kid: impl Into<String>, seed: &[u8; 32]) -> Self {
        Self {
            kid: kid.into(),
            key: SigningKey::from_bytes(seed),
            max_records: DEFAULT_MAX_RECORDS_PER_TENANT,
            max_tenants: DEFAULT_MAX_TENANTS,
            state: Mutex::new(State::default()),
        }
    }

    /// Keep at most `max_records` records **per tenant**; a tenant at its bound gets
    /// `Unavailable` for a new record while the others are not affected. A key already signed
    /// is still served.
    #[must_use]
    pub fn with_max_records_per_tenant(mut self, max_records: usize) -> Self {
        self.max_records = max_records;
        self
    }

    /// Follow at most `max_tenants` tenants; a record of a further tenant is `Unavailable`.
    #[must_use]
    pub fn with_max_tenants(mut self, max_tenants: usize) -> Self {
        self.max_tenants = max_tenants;
        self
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The raw 32-byte public key, to pin in a verifier's [`TrustedKeys`](super::TrustedKeys).
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// How many records this signer holds.
    pub fn len(&self) -> usize {
        // The table is only read here, so a lock poisoned by a panic elsewhere is still readable.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_key
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn sign_now(
        &self,
        proof: &ExecutionProof,
    ) -> Result<SignedExecutionProof, SignerError> {
        check(proof).map_err(SignerError::Refused)?;
        let payload =
            payload_hash(proof).ok_or(SignerError::Refused(SignerRefusal::InvalidProof))?;
        let mut state = self.state.lock().map_err(|_| SignerError::Unavailable)?;
        let slot = (proof.tenant_id.clone(), proof.idempotency_key.clone());
        if let Some(existing) = state.by_key.get(&slot) {
            return if existing.proof.same_content_as(proof) {
                Ok(existing.clone())
            } else {
                Err(SignerError::Refused(SignerRefusal::Conflict))
            };
        }
        match state.per_tenant.get(&proof.tenant_id) {
            Some(held) if *held >= self.max_records => return Err(SignerError::Unavailable),
            None if state.per_tenant.len() >= self.max_tenants => {
                return Err(SignerError::Unavailable)
            }
            _ => {}
        }
        let chain_id = (proof.tenant_id.clone(), proof.run_id.clone());
        let prev = state.tails.get(&chain_id).cloned();
        let signature = self.key.sign(&signing_message(&payload, prev.as_deref()));
        let record = SignedExecutionProof {
            proof: proof.clone(),
            algorithm: PROOF_ALGORITHM.to_owned(),
            domain: PROOF_DOMAIN.to_owned(),
            kid: self.kid.clone(),
            prev_hash: prev,
            payload_hash: payload.clone(),
            signature: STANDARD.encode(signature.to_bytes()),
        };
        state.tails.insert(chain_id, payload);
        *state.per_tenant.entry(proof.tenant_id.clone()).or_insert(0) += 1;
        state.by_key.insert(slot, record.clone());
        Ok(record)
    }
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What the signer can check about a proof without the guard's help.
fn check(proof: &ExecutionProof) -> Result<(), SignerRefusal> {
    let digests = [
        &proof.request_digest,
        &proof.profile_digest,
        &proof.inputs_digest,
        &proof.outputs_digest,
        &proof.stdout_digest,
        &proof.stderr_digest,
        &proof.approval_payload_hash,
    ];
    let named = [
        &proof.run_id,
        &proof.node_id,
        &proof.tenant_id,
        &proof.sandbox_id,
        &proof.idempotency_key,
        &proof.isolation_domain,
        &proof.approval.call_key,
        &proof.approval.approved_by,
    ];
    if proof.kind != PROOF_KIND
        || digests.iter().any(|d| !is_hex64(d))
        || named.iter().any(|n| n.trim().is_empty())
        || proof.started_at > proof.finished_at
    {
        return Err(SignerRefusal::InvalidProof);
    }
    Ok(())
}

#[async_trait]
impl ProofSigner for InProcessProofSigner {
    async fn sign(&self, proof: &ExecutionProof) -> Result<SignedExecutionProof, SignerError> {
        self.sign_now(proof)
    }
}
