//! The execution proof and the port that signs it.
//!
//! An [`ExecutionProof`] is a canonical record of **digests only**: it never holds output
//! bytes, an argument or a secret. After `collect`, the guard builds it from what it recomputed
//! over the bytes it received, and hands it to a [`ProofSigner`]. The signer returns a
//! [`SignedExecutionProof`] chained to the previous proof of the same run. Without a signed
//! proof, a guard that has a signer returns no result (see
//! [`GovernedSandbox::with_proof_signer`](crate::GovernedSandbox::with_proof_signer)).
//!
//! # Two chains
//!
//! The approval attestation has a frozen shape and its verifier would reject a mixed chain. The
//! proof uses the same primitives (JavaScript-style canonical JSON, Ed25519, `prev_hash`) on a
//! separate **sandbox chain per run**, with a record whose `kind` is `"sandbox_execution"` and
//! is signed with the rest. Each proof carries the `approval_payload_hash` that links it to the
//! approval chain.
//!
//! # What a proof proves
//!
//! The guard recomputes the digests of the bytes it receives, so it detects a gap between a
//! declared digest and the delivered bytes, and any later alteration of the delivered bytes.
//! It does not detect a backend that lies consistently about content, and it does not check the
//! backend's isolation profile: the values named `declared_*`, the isolation domain and the
//! profile digest are **declared**. The signature proves the consistency between the guard's
//! report, the approval and the delivered bytes. Present it as a "consistent proof", never as
//! "executed in verified isolation".
//!
//! The attempt number and the step (`create`, `exec`, `collect`) are not fields of a proof. They
//! enter it only through the idempotency key, which is a hash over the context, the attempt and
//! the step: two attempts of a node are two proofs under two keys, and a verifier that needs the
//! attempt must keep it with the context it hashed, not read it from the record.
//!
//! # Failing closed
//!
//! The guard fails closed when the signer is unavailable or refuses. It cannot tell that a
//! signer signed badly: it holds no key and checks no signature, only that the record it gets
//! back is coherent and about the proof it submitted. The signature is checked by the verifier,
//! against keys the verifier pinned.
//!
//! # Keys
//!
//! A record carries the `kid` of the key that signed it and never the key. A verifier checks
//! against keys it pinned itself ([`TrustedKeys`]), with rotation (several `kid` valid at once)
//! and revocation. Rotating the **signing** key starts a new chain: the first record under the
//! new key has no `prev_hash`, so [`verify_chain`] over the concatenation of the old and the new
//! records fails with `BrokenLink`. That is visible, not silent: verify each key's chain apart.
//! The in-process signer does not erase the seed its caller supplied (the caller owns that
//! array); it erases its own signing key when dropped.
//!
//! # Signing domain
//!
//! A proof signature covers [`PROOF_SIGN_DOMAIN`] followed by the hex text of the chain hash,
//! and every record states its `domain` ([`PROOF_DOMAIN`]); a verifier refuses another value.
//! The approval attestation signs the bare chain hash, so a signature of one chain never
//! verifies in the other, even under the same key. Use a key per domain all the same: the label
//! is a second defense, not a reason to share a key.
//!
//! # Whose run?
//!
//! [`verify_proof`], [`verify_chain`] and their variants without `_for` check that a record or a
//! chain is **authentic** (a pinned key signed it) and **internally coherent** (links, one
//! tenant and run, no repeated `collect`). They do not check that it is the record or chain of a
//! given tenant and run: a valid chain of tenant B passes for a verifier that audits tenant A.
//! A verifier that audits a run uses [`verify_proof_for`], [`verify_chain_for`] or
//! [`verify_chain_ending_at_for`], passing the tenant and run it expects from its own records.

mod canon;
mod in_process;
mod verify;

#[cfg(test)]
pub(crate) mod tests;

pub use canon::{canonical_json, canonical_proof_json, chain_hash, payload_hash, signing_message};
pub use in_process::{InProcessProofSigner, DEFAULT_MAX_RECORDS, DEFAULT_MAX_TENANTS};
pub use verify::{
    verify_chain, verify_chain_ending_at, verify_chain_ending_at_for, verify_chain_for,
    verify_chain_values, verify_proof, verify_proof_for, verify_proof_value, ChainError,
    ChainFault, KeyError, ProofError, TrustedKeys,
};

use crate::limits::Limits;
use crate::outcome::DeclaredExit;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The `kind` of every sandbox proof. It is signed: a record with another `kind` is not a
/// sandbox proof, whatever else it holds.
pub const PROOF_KIND: &str = "sandbox_execution";

/// The only signature algorithm of a proof record.
pub const PROOF_ALGORITHM: &str = "ed25519";

/// The domain label of the signature of a proof record, versioned. It is the `domain` of every
/// record, so that a later signing format can be told from this one.
pub const PROOF_DOMAIN: &str = "ailu.sandbox.proof.v1";

/// What is put in front of the message a proof signature covers: [`PROOF_DOMAIN`] and a NUL
/// byte. The signed bytes are `PROOF_SIGN_DOMAIN` followed by the 64 lowercase hex characters of
/// the [`chain_hash`]. The approval attestation signs that hex text bare, so a signature made
/// for one never verifies for the other, even under the same key. A key per domain is still
/// the rule: the label is a second defense, not a reason to share a key.
pub const PROOF_SIGN_DOMAIN: &[u8] = b"ailu.sandbox.proof.v1\0";

/// The approval that authorized the call, as the grant verifier established it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProofApproval {
    /// The call key the approval is bound to (`sandbox.exec#<sha256>`).
    pub call_key: String,
    /// The principal that approved the call, as the verifier returned it.
    pub approved_by: String,
}

/// The canonical record of one execution: digests and identifiers only, never content.
///
/// Every field is signed. Values that the guard recomputed (the stream and output digests, the
/// request digest, the times it read from its clock) are the guard's own; the values named
/// `declared_*`, `isolation_domain` and `profile_digest` are **declared** by the backend or by
/// the request and are not independently verified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionProof {
    /// Always [`PROOF_KIND`]. Signed, so a proof cannot be passed off as another record.
    pub kind: String,
    pub run_id: String,
    pub node_id: String,
    pub tenant_id: String,
    pub sandbox_id: String,
    /// The idempotency key of the `collect` this proof is about (lowercase hex). A signer is
    /// idempotent by this value.
    pub idempotency_key: String,
    /// The isolation domain, **as the backend declared it**.
    pub isolation_domain: String,
    /// Digest of the whole call: the `create`, `exec` and `collect` requests.
    pub request_digest: String,
    /// SHA-256 of the isolation profile name that was asked for. **Declared**: the guard cannot
    /// see how the deployment resolved it.
    pub profile_digest: String,
    /// The limits the guard validated and passed to the backend.
    pub limits_effective: Limits,
    /// Digest of the input artifact references and their destinations.
    pub inputs_digest: String,
    /// Digest over the sorted `(path, sha256)` pairs of the delivered files, recomputed by the
    /// guard over the bytes it holds (after redaction).
    pub outputs_digest: String,
    /// SHA-256 of the delivered standard output (after redaction), recomputed by the guard.
    pub stdout_digest: String,
    /// SHA-256 of the delivered standard error (after redaction), recomputed by the guard.
    pub stderr_digest: String,
    /// Whether the backend cut an output stream or the collection at a bound.
    pub truncated: bool,
    /// How the program ended, as the backend declared it.
    pub declared_exit: DeclaredExit,
    /// How long the program ran, as the backend declared it.
    pub declared_duration_ms: u64,
    /// When the guard sent the program to the backend, in milliseconds since the Unix epoch.
    /// Read from the injected [`SandboxClock`](crate::SandboxClock) just before the backend
    /// call (on each attempt) and adjusted by the guard's own monotonic time, so a clock that
    /// stepped back does not move it back.
    pub started_at: u64,
    /// When the backend answered, on the same time as `started_at`, and never before it
    /// (`finished_at >= started_at`). A reading the guard refuses to take, such as an
    /// aberrant jump, is not used: the end is then dated like the start.
    pub finished_at: u64,
    /// An isolation violation was detected. A sandbox with a violation is poisoned and cannot
    /// be collected, so a proof the guard builds today always has `false` here.
    pub violation: bool,
    pub approval: ProofApproval,
    /// Payload hash of the approval record that authorized the call (64 lowercase hex
    /// characters): the link to the approval chain.
    pub approval_payload_hash: String,
}

impl ExecutionProof {
    /// Whether `other` says the same thing about the same execution, leaving out the two times.
    ///
    /// A `collect` replayed after the host restarted dates the execution again, so its proof
    /// differs from the signed one by the times only. The signed record, with the times of the
    /// first signing, is the one that stands.
    pub fn same_content_as(&self, other: &ExecutionProof) -> bool {
        let mut other = other.clone();
        other.started_at = self.started_at;
        other.finished_at = self.finished_at;
        *self == other
    }
}

/// A proof signed and chained by a [`ProofSigner`].
///
/// The signature covers [`chain_hash`] of `payload_hash` and `prev_hash`; `payload_hash` is the
/// hash of the canonical JSON of `proof` ([`payload_hash`]). The record carries the `kid` of
/// the signing key, never the key: a verifier uses keys it pinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedExecutionProof {
    pub proof: ExecutionProof,
    /// Always [`PROOF_ALGORITHM`].
    pub algorithm: String,
    /// Always [`PROOF_DOMAIN`]: the version of what the signature covers. A verifier refuses
    /// another value ([`ProofError::UnsupportedDomain`](ProofError::UnsupportedDomain)) and a
    /// record that has none.
    pub domain: String,
    /// Identifier of the key that signed, in the verifier's pinned set.
    pub kid: String,
    /// `payload_hash` of the previous record of the run's sandbox chain; `None` for the first.
    pub prev_hash: Option<String>,
    /// Lowercase hex SHA-256 of the canonical JSON of `proof`.
    pub payload_hash: String,
    /// Base64 of the 64-byte Ed25519 signature over the chain hash.
    pub signature: String,
}

/// Why a signer refused to sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignerRefusal {
    /// The proof is malformed: another `kind`, a digest that is not a SHA-256, times out of
    /// order, or a value that has no canonical form.
    #[error("the proof is malformed")]
    InvalidProof,
    /// The signer already signed a proof under this idempotency key and it says something else.
    #[error("another proof is already signed under this key")]
    Conflict,
    /// The signer's own rules refuse this proof.
    #[error("the signer refuses this proof")]
    Rejected,
}

/// Why a [`ProofSigner`] returned no signed proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignerError {
    /// The signer cannot answer now (the key service is down, the store is full...). The guard
    /// fails closed with `Unavailable { retriable: true }`; asking again may succeed.
    #[error("proof signer unavailable")]
    Unavailable,
    /// The signer will not sign this proof. The guard fails closed with
    /// `policy_denied.proof_refused`.
    #[error("proof signer refused: {0}")]
    Refused(SignerRefusal),
}

/// The port that signs execution proofs. The guard builds the proof and submits it; the
/// deployment chooses the implementation. A backend never signs, and the guard holds no key.
///
/// # Contract
///
/// - **Idempotent by `proof.idempotency_key`.** Asked again for a proof with the same key and
///   the same content ([`ExecutionProof::same_content_as`]), a signer returns the record it
///   already signed, byte for byte, and appends nothing to the chain. With another content it
///   refuses ([`SignerRefusal::Conflict`]); it never signs two records under one key.
/// - **One writer per run.** The signer holds the order of each run's sandbox chain: every
///   record it signs for a run has the previous record's `payload_hash` as `prev_hash`, and
///   calls for the same run never fork the chain, however many are in flight.
/// - **May re-verify.** Before signing, a signer may check what it can (tenant and run, that
///   the approval is bound to the call, the shape of the digests) and refuse.
/// - **Returns the proof it was given.** The record's `proof` is the submitted one (with the
///   times of the first signing if the key was already signed). The guard refuses a record
///   that says anything else.
/// - **Typed failure.** A signer that cannot answer returns [`SignerError::Unavailable`], one
///   that will not sign returns [`SignerError::Refused`]. It never returns an unsigned record.
///
/// The conformance suite (`conformance` feature) checks these properties.
#[async_trait]
pub trait ProofSigner: Send + Sync {
    async fn sign(&self, proof: &ExecutionProof) -> Result<SignedExecutionProof, SignerError>;
}
