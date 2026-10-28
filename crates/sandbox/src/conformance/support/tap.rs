//! The tap: a [`SandboxBackend`] that sits between the guard and the target's backend. It
//! counts what the guard asks, can hold a call, and can fail or rewrite an answer once, so
//! that the suite makes any backend misbehave in the ways the guard must withstand.

use crate::backend::SandboxBackend;
use crate::cancel::CancelToken;
use crate::collected::RawCollected;
use crate::error::SandboxError;
use crate::ids::{IdempotencyKey, RawCreated, SandboxContext, SandboxHandle, SandboxStep};
use crate::journal::{Appended, InMemoryJournal, JournalError, SandboxEvent, SandboxJournal};
use crate::outcome::ExecOutcome;
use crate::request::{CollectRequest, CreateRequest, ExecRequest};
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Waker;

type Rewrite<T> = Box<dyn FnOnce(Result<T, SandboxError>) -> Result<T, SandboxError> + Send>;

/// A call held at the tap until it is opened.
pub(crate) struct Gate {
    state: Mutex<(bool, Vec<Waker>)>,
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((false, Vec::new())),
        })
    }

    /// Let every call held here go on.
    pub(crate) fn open(&self) {
        let wakers = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0 {
                std::task::Poll::Ready(())
            } else {
                state.1.push(cx.waker().clone());
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

#[derive(Default)]
struct State {
    calls: Vec<(SandboxStep, String)>,
    last_create: Option<CreateRequest>,
    before: HashMap<SandboxStep, VecDeque<SandboxError>>,
    gates: HashMap<SandboxStep, Arc<Gate>>,
    create_after: VecDeque<Rewrite<RawCreated>>,
    exec_after: VecDeque<Rewrite<ExecOutcome>>,
    collect_after: VecDeque<Rewrite<RawCollected>>,
    on_exec: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Inner {
    backend: Arc<dyn SandboxBackend>,
    state: Mutex<State>,
}

/// The tap. Cheap to clone: clones share one state, so a second guard over a clone is the same
/// backend after a restart of the host.
#[derive(Clone)]
pub(crate) struct Tap(Arc<Inner>);

impl Tap {
    pub(crate) fn new(backend: Arc<dyn SandboxBackend>) -> Self {
        Self(Arc::new(Inner {
            backend,
            state: Mutex::new(State::default()),
        }))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many times the guard asked the backend to do `step`.
    pub(crate) fn count(&self, step: SandboxStep) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|(s, _)| *s == step)
            .count()
    }

    /// How many times the guard asked the backend anything.
    pub(crate) fn total(&self) -> usize {
        self.state().calls.len()
    }

    /// The steps the guard asked, in order.
    pub(crate) fn steps(&self) -> Vec<SandboxStep> {
        self.state().calls.iter().map(|(step, _)| *step).collect()
    }

    /// The `create` request the backend last received, as the guard passed it.
    pub(crate) fn last_create(&self) -> Option<CreateRequest> {
        self.state().last_create.clone()
    }

    /// Answer the next call of `step` with `error` without reaching the backend.
    pub(crate) fn fail_next(&self, step: SandboxStep, error: SandboxError) {
        self.state()
            .before
            .entry(step)
            .or_default()
            .push_back(error);
    }

    /// Hold every call of `step` until the returned gate is opened.
    pub(crate) fn hold(&self, step: SandboxStep) -> Arc<Gate> {
        let gate = Gate::new();
        self.state().gates.insert(step, Arc::clone(&gate));
        gate
    }

    /// Run `f` when an `exec` reaches the backend, before the backend sees it.
    pub(crate) fn on_exec(&self, f: Arc<dyn Fn() + Send + Sync>) {
        self.state().on_exec = Some(f);
    }

    /// Rewrite the answer of the next `create`.
    pub(crate) fn rewrite_create(&self, f: Rewrite<RawCreated>) {
        self.state().create_after.push_back(f);
    }

    /// Rewrite the answer of the next `exec`.
    pub(crate) fn rewrite_exec(&self, f: Rewrite<ExecOutcome>) {
        self.state().exec_after.push_back(f);
    }

    /// Rewrite the answer of the next `collect`.
    pub(crate) fn rewrite_collect(&self, f: Rewrite<RawCollected>) {
        self.state().collect_after.push_back(f);
    }

    /// Note the call; return the error scripted for it, and the gate that holds it.
    fn enter(
        &self,
        step: SandboxStep,
        key: &IdempotencyKey,
    ) -> (Option<SandboxError>, Option<Arc<Gate>>) {
        let mut state = self.state();
        state.calls.push((step, key.as_str().to_owned()));
        let error = state.before.get_mut(&step).and_then(VecDeque::pop_front);
        let gate = state.gates.get(&step).cloned();
        (error, gate)
    }
}

#[async_trait]
impl SandboxBackend for Tap {
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError> {
        let (error, gate) = self.enter(SandboxStep::Create, key);
        self.state().last_create = Some(req.clone());
        if let Some(error) = error {
            return Err(error);
        }
        if let Some(gate) = gate {
            gate.wait().await;
        }
        let result = self.0.backend.create(ctx, key, req, cancel).await;
        let rewrite = self.state().create_after.pop_front();
        match rewrite {
            Some(f) => f(result),
            None => result,
        }
    }

    async fn exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError> {
        let (error, gate) = self.enter(SandboxStep::Exec, key);
        let on_exec = self.state().on_exec.clone();
        if let Some(on_exec) = on_exec {
            on_exec();
        }
        if let Some(error) = error {
            return Err(error);
        }
        if let Some(gate) = gate {
            gate.wait().await;
        }
        let result = self.0.backend.exec(h, key, req, cancel).await;
        let rewrite = self.state().exec_after.pop_front();
        match rewrite {
            Some(f) => f(result),
            None => result,
        }
    }

    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError> {
        let (error, gate) = self.enter(SandboxStep::Collect, key);
        if let Some(error) = error {
            return Err(error);
        }
        if let Some(gate) = gate {
            gate.wait().await;
        }
        let result = self.0.backend.collect(h, key, req, cancel).await;
        let rewrite = self.state().collect_after.pop_front();
        match rewrite {
            Some(f) => f(result),
            None => result,
        }
    }

    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError> {
        let (error, gate) = self.enter(SandboxStep::Destroy, key);
        if let Some(error) = error {
            return Err(error);
        }
        if let Some(gate) = gate {
            gate.wait().await;
        }
        self.0.backend.destroy(h, key).await
    }
}

/// The guard's journal in the suite: in memory, and able to fail on demand.
#[derive(Default)]
pub(crate) struct TestJournal {
    inner: InMemoryJournal,
    fail: AtomicBool,
}

impl TestJournal {
    pub(crate) fn set_failing(&self, failing: bool) {
        self.fail.store(failing, Ordering::SeqCst);
    }

    pub(crate) fn events(&self) -> Vec<SandboxEvent> {
        self.inner.events()
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }
}

#[async_trait]
impl SandboxJournal for TestJournal {
    async fn append(&self, event: SandboxEvent) -> Result<Appended, JournalError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(JournalError);
        }
        self.inner.append(event).await
    }
}
