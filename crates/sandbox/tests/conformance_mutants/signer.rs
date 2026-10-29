//! Signers that break one obligation each, put in front of a correct guard and the simulator.
//! The `proof` group must notice. The honest signer here is written from the public API alone
//! (the canonical form, the hashes, Ed25519), so the defects are not hidden in the crate.

use ailu_sandbox::conformance::{
    ConformanceTarget, Instance, ProbeKind, ProbeProgram, ProofKit, SimTarget,
};
use ailu_sandbox::proof::{
    canonical_json, chain_hash, payload_hash, ExecutionProof, InProcessProofSigner, ProofSigner,
    SignedExecutionProof, SignerError, SignerRefusal, TrustedKeys, PROOF_ALGORITHM, PROOF_KIND,
};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

const SEED: [u8; 32] = [0x31; 32];
const OTHER_SEED: [u8; 32] = [0x66; 32];
const KID: &str = "mutant-key";

/// What a signer gets wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerMutation {
    /// Not a defect: the signer written here, honest. Every case must pass.
    Honest,
    /// Asked again for the same key, it signs a new record and extends the chain.
    NotIdempotent,
    /// It signs whatever it is given, a proof of another kind or with malformed digests.
    SignsAnything,
    /// Every record is the first of its chain.
    ForgetsPrev,
    /// It signs with a key the verifier did not pin.
    UnpinnedKey,
    /// The hash it signs leaves out the kind.
    KindUnsigned,
    /// Another content under a key already signed gets the record signed first.
    ConflictIgnored,
    /// It reads the end of the chain, gives the executor back, and only then signs: concurrent
    /// calls fork the chain.
    ForkedChain,
    /// It signs every proof twice, chains both, and hands back the second: the first stays in
    /// the chain unseen.
    DuplicateInChain,
    /// It signs a proof with another digest of the standard output than the one submitted.
    SignsOtherContent,
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn well_formed(proof: &ExecutionProof) -> bool {
    proof.kind == PROOF_KIND
        && [
            &proof.request_digest,
            &proof.profile_digest,
            &proof.inputs_digest,
            &proof.outputs_digest,
            &proof.stdout_digest,
            &proof.stderr_digest,
            &proof.approval_payload_hash,
        ]
        .iter()
        .all(|d| is_hex64(d))
        && ![&proof.run_id, &proof.node_id, &proof.tenant_id]
            .iter()
            .any(|n| n.trim().is_empty())
        && proof.started_at <= proof.finished_at
}

#[derive(Default)]
struct State {
    by_key: HashMap<String, SignedExecutionProof>,
    tails: HashMap<(String, String), String>,
}

pub struct MutantSigner {
    mutation: SignerMutation,
    key: SigningKey,
    kid: &'static str,
    state: Mutex<State>,
}

/// Gives the executor back once.
async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

impl MutantSigner {
    fn new(mutation: SignerMutation) -> Self {
        let (seed, kid) = match mutation {
            SignerMutation::UnpinnedKey => (OTHER_SEED, "unpinned-key"),
            _ => (SEED, KID),
        };
        Self {
            mutation,
            key: SigningKey::from_bytes(&seed),
            kid,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, proof: &ExecutionProof, prev: Option<String>) -> SignedExecutionProof {
        let payload = if self.mutation == SignerMutation::KindUnsigned {
            // The canonical JSON of the proof with its kind taken out.
            let mut value = serde_json::to_value(proof).unwrap_or_default();
            if let Some(object) = value.as_object_mut() {
                object.remove("kind");
            }
            let text = canonical_json(&value).unwrap_or_default();
            format!("{:x}", Sha256::digest(text.as_bytes()))
        } else {
            payload_hash(proof).unwrap_or_default()
        };
        let chain = chain_hash(&payload, prev.as_deref());
        SignedExecutionProof {
            proof: proof.clone(),
            algorithm: PROOF_ALGORITHM.to_owned(),
            kid: self.kid.to_owned(),
            prev_hash: prev,
            payload_hash: payload,
            signature: STANDARD.encode(self.key.sign(chain.as_bytes()).to_bytes()),
        }
    }

    fn tail(&self, proof: &ExecutionProof) -> Option<String> {
        if self.mutation == SignerMutation::ForgetsPrev {
            return None;
        }
        self.state()
            .tails
            .get(&(proof.tenant_id.clone(), proof.run_id.clone()))
            .cloned()
    }

    fn push(&self, proof: &ExecutionProof, record: &SignedExecutionProof) {
        let mut state = self.state();
        state.tails.insert(
            (proof.tenant_id.clone(), proof.run_id.clone()),
            record.payload_hash.clone(),
        );
        state
            .by_key
            .insert(proof.idempotency_key.clone(), record.clone());
    }
}

#[async_trait]
impl ProofSigner for MutantSigner {
    async fn sign(&self, proof: &ExecutionProof) -> Result<SignedExecutionProof, SignerError> {
        use SignerMutation as M;
        if self.mutation != M::SignsAnything && !well_formed(proof) {
            return Err(SignerError::Refused(SignerRefusal::InvalidProof));
        }
        let existing = self.state().by_key.get(&proof.idempotency_key).cloned();
        if let Some(existing) = existing {
            match self.mutation {
                M::NotIdempotent => {}
                M::ConflictIgnored => return Ok(existing),
                _ => {
                    return if existing.proof.same_content_as(proof) {
                        Ok(existing)
                    } else {
                        Err(SignerError::Refused(SignerRefusal::Conflict))
                    };
                }
            }
        }
        let signed = match self.mutation {
            M::SignsOtherContent => {
                let mut other = proof.clone();
                other.stdout_digest = format!("{:x}", Sha256::digest(b"another output"));
                let prev = self.tail(&other);
                self.record(&other, prev)
            }
            M::DuplicateInChain => {
                let prev = self.tail(proof);
                let hidden = self.record(proof, prev);
                self.push(proof, &hidden);
                self.record(proof, Some(hidden.payload_hash))
            }
            M::ForkedChain => {
                let prev = self.tail(proof);
                yield_once().await;
                self.record(proof, prev)
            }
            _ => {
                let prev = self.tail(proof);
                self.record(proof, prev)
            }
        };
        self.push(proof, &signed);
        Ok(signed)
    }
}

/// The simulator with a defective signer; the keys the verifier pins are the honest ones.
pub struct SignerMutantTarget {
    base: SimTarget,
    mutation: SignerMutation,
}

impl SignerMutantTarget {
    pub fn new(mutation: SignerMutation) -> Self {
        Self {
            base: SimTarget::new(),
            mutation,
        }
    }
}

#[async_trait]
impl ConformanceTarget for SignerMutantTarget {
    type Backend = ailu_sandbox::sim::SimBackend;

    fn name(&self) -> String {
        format!("signer mutant {:?}", self.mutation)
    }

    async fn fresh(&self) -> Instance<Self::Backend> {
        let mut instance = self.base.fresh().await;
        let honest = InProcessProofSigner::from_seed(KID, &SEED);
        let mut trusted = TrustedKeys::new();
        trusted
            .trust(KID, &honest.public_key())
            .expect("a derived key is valid");
        instance.proof = Some(ProofKit {
            signer: Arc::new(MutantSigner::new(self.mutation)),
            trusted,
        });
        instance
    }

    fn probe(&self, kind: &ProbeKind) -> Option<ProbeProgram> {
        self.base.probe(kind)
    }
}
