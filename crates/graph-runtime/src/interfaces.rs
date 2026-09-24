//! Engine seams: handlers, registries, checkpointer, event bus — and their
//! in-memory implementations. Mirrors the TS interfaces; node execution is
//! synchronous in this first slice (async/LLM lands with the agent crates).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use ailu_graph_core::{FailureCategory, GraphState, NodeId, RunId};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::types::{Checkpoint, CheckpointId, RunEvent};

/// A handler-raised interrupt: suspends the run during the node, applying `patch`
/// to the channels. On resume the node re-runs — the analogue of the TS
/// `DynamicInterrupt` (e.g. an agent waiting on human approval).
#[derive(Clone, Debug, PartialEq)]
pub struct Interrupt {
    pub reason: String,
    pub patch: BTreeMap<String, Value>,
}

/// What a node returns: a partial channel update, optionally overriding routing,
/// an interrupt that suspends the run, a durable timer / signal wait, or a failure
/// that triggers the node's retry policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeOutput {
    pub update: BTreeMap<String, Value>,
    /// Explicit routing target(s). `None` lets the runtime follow the graph's edges.
    pub goto: Option<Vec<NodeId>>,
    /// When set, the run suspends here instead of advancing.
    pub interrupt: Option<Interrupt>,
    /// When set, the attempt failed: the update is discarded, routing does not
    /// advance, and the runtime retries per the node's `retryPolicy` (the TS
    /// analogue of a handler throwing).
    pub failure: Option<String>,
    /// ADR 0076 — optional classification of `failure`, set via
    /// `NodeOutput::failure_with_category`. `None` (the common case, e.g. plain
    /// `NodeOutput::failure(reason)`) is treated as `FailureCategory::Unknown` at
    /// emission time.
    pub failure_category: Option<FailureCategory>,
    /// Durable timer: when set, the run applies this node's update, then SUSPENDS until
    /// an external scheduler resumes it — `wake_at` (an opaque deadline string, e.g.
    /// ISO-8601 / epoch-millis) is **data**, the engine never reads a clock or sleeps.
    /// Unlike [`Interrupt`], resume ADVANCES past this node (a one-shot sleep, like a
    /// gate opening). The embedder's scheduler (the control-plane worker) reads `wake_at`
    /// from the suspended run and calls `resume` at that time.
    pub sleep_until: Option<String>,
    /// External signal wait: when set, the run applies this node's update, then SUSPENDS
    /// awaiting a named signal. [`GraphRuntime::resume_with_signal`] injects the signal's
    /// payload and advances past this node. Combine with [`Self::sleep_until`] for a
    /// signal-or-timeout (whichever resume fires first; downstream inspects which channel
    /// was populated).
    pub wait_for_signal: Option<String>,
}

impl NodeOutput {
    /// An update-only output (follow the declared edges).
    pub fn update(update: BTreeMap<String, Value>) -> Self {
        NodeOutput {
            update,
            ..NodeOutput::default()
        }
    }

    /// An output that suspends the run with the given reason and state patch.
    pub fn interrupt(reason: impl Into<String>, patch: BTreeMap<String, Value>) -> Self {
        NodeOutput {
            interrupt: Some(Interrupt {
                reason: reason.into(),
                patch,
            }),
            ..NodeOutput::default()
        }
    }

    /// An output signalling that this attempt failed with `reason`. Its update is
    /// never applied and routing never advances; the runtime honours the node's
    /// `retryPolicy` and fails the run once attempts are exhausted.
    pub fn failure(reason: impl Into<String>) -> Self {
        NodeOutput {
            failure: Some(reason.into()),
            ..NodeOutput::default()
        }
    }

    /// ADR 0076 — a failure the handler classifies itself (e.g. an HTTP-calling component
    /// distinguishing a 429 as `Transient`). Any other failure defaults to `Unknown`.
    pub fn failure_with_category(reason: impl Into<String>, category: FailureCategory) -> Self {
        NodeOutput {
            failure: Some(reason.into()),
            failure_category: Some(category),
            ..NodeOutput::default()
        }
    }

    /// A durable-timer output: suspend until `wake_at`, then advance. `wake_at` is an
    /// opaque deadline (the engine never reads a clock); the embedder's scheduler
    /// resumes the run at that time.
    pub fn sleep(wake_at: impl Into<String>) -> Self {
        NodeOutput {
            sleep_until: Some(wake_at.into()),
            ..NodeOutput::default()
        }
    }

    /// A signal-wait output: suspend until a `resume_with_signal(name, payload)`, then
    /// advance with the payload injected into the `__signals` channel.
    pub fn wait_for_signal(name: impl Into<String>) -> Self {
        NodeOutput {
            wait_for_signal: Some(name.into()),
            ..NodeOutput::default()
        }
    }

    /// A signal-OR-timeout output: suspend awaiting `name`, but also wake at `wake_at`
    /// if the signal never arrives. Downstream inspects whether the signal channel was
    /// populated to tell which fired.
    pub fn wait_for_signal_or_timeout(name: impl Into<String>, wake_at: impl Into<String>) -> Self {
        NodeOutput {
            wait_for_signal: Some(name.into()),
            sleep_until: Some(wake_at.into()),
            ..NodeOutput::default()
        }
    }
}

/// A boxed future produced by an async node handler.
pub type BoxFuture = Pin<Box<dyn Future<Output = NodeOutput> + Send>>;

/// A node handler: async over the current state, producing an output. Takes the
/// state by value so the returned future is `'static` (handlers may do real I/O —
/// LLM calls, tools — once the agent crates land).
pub type NodeHandler = Box<dyn Fn(GraphState) -> BoxFuture + Send + Sync>;

/// Wrap a synchronous closure as an async node handler — convenient for pure nodes
/// (and tests) that don't await anything.
pub fn sync_handler<F>(f: F) -> NodeHandler
where
    F: Fn(GraphState) -> NodeOutput + Send + Sync + 'static,
{
    Box::new(move |state| Box::pin(std::future::ready(f(state))))
}

/// A named condition predicate. Conditions are never `eval`'d strings.
pub type ConditionFn = Box<dyn Fn(&GraphState) -> bool + Send + Sync>;

/// A named condition predicate whose evaluation can fail — e.g. one answered by a host callback
/// that may throw. A failure fails the run; it is never read as `false` (which would silently
/// route the run down another branch).
pub type FallibleConditionFn = Box<dyn Fn(&GraphState) -> Result<bool, String> + Send + Sync>;

/// Trait objects for the engine seams are required to be `Send + Sync` so the
/// `GraphRuntime` run future stays `Send` (it is driven from a napi async fn,
/// whose future must be `Send`). The supertrait bound makes a non-thread-safe
/// implementation fail to compile at the boundary rather than at the call site.
pub trait NodeRegistry: Send + Sync {
    fn register(&mut self, node_id: NodeId, handler: NodeHandler);
    fn resolve(&self, node_id: &NodeId) -> Option<&NodeHandler>;
}

pub trait ConditionRegistry: Send + Sync {
    fn register(&mut self, name: String, predicate: ConditionFn);
    fn resolve(&self, name: &str) -> Option<&ConditionFn>;
}

pub trait Checkpointer: Send + Sync {
    fn save(&self, checkpoint: Checkpoint);
    fn load(&self, run_id: &RunId) -> Option<Checkpoint>;
    /// Look up a single checkpoint by its id, across all runs (time-travel seam).
    fn load_by_id(&self, id: &CheckpointId) -> Option<Checkpoint>;
    fn list(&self, run_id: &RunId) -> Vec<Checkpoint>;
}

pub trait EventBus: Send + Sync {
    fn emit(&self, event: RunEvent);
}

/// The runtime's ONLY source of wall-clock time. Every durable timestamp
/// (`GraphState.created_at`/`updated_at`, `Checkpoint.created_at`) and every
/// `RunEvent.timestamp` flows through here. Ids are already deterministic (seq-based),
/// so this is the sole non-determinism seam — making it injectable is what lets a
/// replay re-feed the original run's timestamps (replay-as-evidence, ADR 0038).
pub trait Clock: Send + Sync {
    fn now_string(&self) -> String;
}

/// Real wall clock — millis since the Unix epoch, as a String. The default; its body is
/// byte-identical to the previous free `now_string()`, so live runs are unchanged.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_string(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        millis.to_string()
    }
}

/// Replays a recorded sequence of timestamps in call order. When the sequence is
/// exhausted it CLAMPS (repeats the last value) instead of panicking, so a small drift
/// in `now_string()` call count between record and replay is tolerated — timestamps are
/// evidence-cosmetic, not the faithfulness signal (which is the decision set).
pub struct RecordedClock {
    values: Vec<String>,
    cursor: AtomicUsize,
}

impl RecordedClock {
    pub fn new(values: Vec<String>) -> Self {
        Self {
            values,
            cursor: AtomicUsize::new(0),
        }
    }
}

impl Clock for RecordedClock {
    fn now_string(&self) -> String {
        if self.values.is_empty() {
            return "0".to_string();
        }
        let i = self.cursor.fetch_add(1, Ordering::SeqCst);
        let idx = i.min(self.values.len() - 1);
        self.values[idx].clone()
    }
}

/// Wraps another [`Clock`] (typically [`SystemClock`]) and RECORDS every `now_string()` into
/// a shared buffer in call order — so a record-mode run captures the exact timestamp sequence
/// a later [`RecordedClock`] replays (ADR 0038). Transparent: returns the inner value unchanged.
pub struct RecordingClock {
    inner: Arc<dyn Clock>,
    recorded: Arc<Mutex<Vec<String>>>,
}

impl RecordingClock {
    pub fn new(inner: Arc<dyn Clock>, recorded: Arc<Mutex<Vec<String>>>) -> Self {
        Self { inner, recorded }
    }
}

impl Clock for RecordingClock {
    fn now_string(&self) -> String {
        let value = self.inner.now_string();
        self.recorded
            .lock()
            .expect("recording clock mutex poisoned")
            .push(value.clone());
        value
    }
}

#[derive(Default)]
pub struct InMemoryNodeRegistry {
    handlers: HashMap<String, NodeHandler>,
}

impl InMemoryNodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl NodeRegistry for InMemoryNodeRegistry {
    fn register(&mut self, node_id: NodeId, handler: NodeHandler) {
        self.handlers.insert(node_id.0, handler);
    }
    fn resolve(&self, node_id: &NodeId) -> Option<&NodeHandler> {
        self.handlers.get(node_id.as_str())
    }
}

#[derive(Default)]
pub struct InMemoryConditionRegistry {
    conditions: HashMap<String, ConditionFn>,
    fallible: HashMap<String, FallibleConditionFn>,
}

impl InMemoryConditionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a predicate whose evaluation can fail (replaces any predicate of that name).
    pub fn register_fallible(&mut self, name: String, predicate: FallibleConditionFn) {
        self.conditions.remove(&name);
        self.fallible.insert(name, predicate);
    }

    /// Evaluate the named condition against `state`: `None` when no predicate of that name is
    /// registered, `Some(Err(..))` when a fallible predicate failed.
    pub fn evaluate(&self, name: &str, state: &GraphState) -> Option<Result<bool, String>> {
        if let Some(predicate) = self.fallible.get(name) {
            return Some(predicate(state));
        }
        self.conditions
            .get(name)
            .map(|predicate| Ok(predicate(state)))
    }
}

impl ConditionRegistry for InMemoryConditionRegistry {
    fn register(&mut self, name: String, predicate: ConditionFn) {
        self.fallible.remove(&name);
        self.conditions.insert(name, predicate);
    }
    fn resolve(&self, name: &str) -> Option<&ConditionFn> {
        self.conditions.get(name)
    }
}

#[derive(Default)]
pub struct InMemoryCheckpointer {
    by_run: Mutex<HashMap<String, Vec<Checkpoint>>>,
}

impl InMemoryCheckpointer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Checkpointer for InMemoryCheckpointer {
    fn save(&self, checkpoint: Checkpoint) {
        self.by_run
            .lock()
            .expect("checkpointer mutex poisoned")
            .entry(checkpoint.run_id.0.clone())
            .or_default()
            .push(checkpoint);
    }
    fn load(&self, run_id: &RunId) -> Option<Checkpoint> {
        self.by_run
            .lock()
            .expect("checkpointer mutex poisoned")
            .get(run_id.as_str())
            .and_then(|list| list.last().cloned())
    }
    fn load_by_id(&self, id: &CheckpointId) -> Option<Checkpoint> {
        self.by_run
            .lock()
            .expect("checkpointer mutex poisoned")
            .values()
            .flat_map(|list| list.iter())
            .find(|checkpoint| checkpoint.id == *id)
            .cloned()
    }
    fn list(&self, run_id: &RunId) -> Vec<Checkpoint> {
        self.by_run
            .lock()
            .expect("checkpointer mutex poisoned")
            .get(run_id.as_str())
            .cloned()
            .unwrap_or_default()
    }
}

/// A live event observer, invoked synchronously for every emitted event. Must be
/// `Send + Sync`: the bus is shared across the (potentially multi-threaded) run
/// future, so observers are too.
pub type EventObserver = Box<dyn Fn(&RunEvent) + Send + Sync>;

#[derive(Default)]
pub struct InMemoryEventBus {
    events: Mutex<Vec<RunEvent>>,
    observers: Mutex<Vec<EventObserver>>,
}

impl InMemoryEventBus {
    pub fn new() -> Self {
        Self::default()
    }
    /// Snapshot of everything emitted so far (for inspection / tests).
    pub fn events(&self) -> Vec<RunEvent> {
        self.events
            .lock()
            .expect("event bus mutex poisoned")
            .clone()
    }
    /// Register a live observer: invoked synchronously, in registration order,
    /// for every subsequent `emit`. This is the embedder-facing streaming seam —
    /// the TS `stream()` modes (values / updates / messages) stay deferred.
    /// Observers must not register further observers from inside the callback.
    pub fn on_event(&self, callback: EventObserver) {
        self.observers
            .lock()
            .expect("event bus mutex poisoned")
            .push(callback);
    }
}

impl EventBus for InMemoryEventBus {
    fn emit(&self, event: RunEvent) {
        // Invoke observers under their own lock, then record the event. Each
        // critical section is tiny and no lock is held across an `.await`
        // (emit is synchronous), so this stays Send-safe.
        for observer in self
            .observers
            .lock()
            .expect("event bus mutex poisoned")
            .iter()
        {
            observer(&event);
        }
        self.events
            .lock()
            .expect("event bus mutex poisoned")
            .push(event);
    }
}
