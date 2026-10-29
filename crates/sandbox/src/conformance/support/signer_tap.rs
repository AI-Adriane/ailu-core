//! The signer tap: a [`ProofSigner`] between the guard and the target's signer. It counts the
//! calls, remembers every record the signer returned, can hold calls, and can fail or rewrite an
//! answer once, so that the suite makes any signer misbehave in the ways the guard must
//! withstand.

use super::tap::Gate;
use crate::proof::{ExecutionProof, ProofSigner, SignedExecutionProof, SignerError};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

type Rewrite = Box<dyn FnOnce(SignedExecutionProof) -> SignedExecutionProof + Send>;

enum Script {
    Fail(SignerError),
    Rewrite(Rewrite),
}

#[derive(Default)]
struct State {
    calls: usize,
    script: VecDeque<Script>,
    gate: Option<Arc<Gate>>,
    returned: Vec<SignedExecutionProof>,
}

pub(crate) struct SignerTap {
    inner: Arc<dyn ProofSigner>,
    state: Mutex<State>,
}

impl SignerTap {
    pub(crate) fn new(inner: Arc<dyn ProofSigner>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many times the guard asked the signer.
    pub(crate) fn calls(&self) -> usize {
        self.state().calls
    }

    /// Answer the next call with `error` without asking the signer.
    pub(crate) fn fail_next(&self, error: SignerError) {
        self.state().script.push_back(Script::Fail(error));
    }

    /// Rewrite the record the signer returns for the next call.
    pub(crate) fn rewrite_next(
        &self,
        f: impl FnOnce(SignedExecutionProof) -> SignedExecutionProof + Send + 'static,
    ) {
        self.state().script.push_back(Script::Rewrite(Box::new(f)));
    }

    /// Hold every call until the returned gate is opened.
    pub(crate) fn hold(&self) -> Arc<Gate> {
        let gate = Gate::new();
        self.state().gate = Some(Arc::clone(&gate));
        gate
    }

    /// Every record the signer returned, in the order it returned them, repeats included.
    pub(crate) fn returned(&self) -> Vec<SignedExecutionProof> {
        self.state().returned.clone()
    }

    /// The records the signer returned, each once (by payload hash).
    pub(crate) fn distinct(&self) -> Vec<SignedExecutionProof> {
        let mut seen = std::collections::HashSet::new();
        self.returned()
            .into_iter()
            .filter(|record| seen.insert(record.payload_hash.clone()))
            .collect()
    }
}

#[async_trait]
impl ProofSigner for SignerTap {
    async fn sign(&self, proof: &ExecutionProof) -> Result<SignedExecutionProof, SignerError> {
        let (scripted, gate) = {
            let mut state = self.state();
            state.calls += 1;
            (state.script.pop_front(), state.gate.clone())
        };
        if let Some(gate) = gate {
            gate.wait().await;
        }
        if let Some(Script::Fail(error)) = scripted {
            return Err(error);
        }
        let mut record = self.inner.sign(proof).await?;
        if let Some(Script::Rewrite(rewrite)) = scripted {
            record = rewrite(record);
        }
        self.state().returned.push(record.clone());
        Ok(record)
    }
}
