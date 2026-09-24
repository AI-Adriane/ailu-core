//! Ailu graph-runtime (Rust).
//!
//! Execution engine over a validated `GraphDefinition` — the Rust port of
//! `@ailu/graph-runtime`. The deterministic, checkpoint-after-every-node,
//! resumable run loop with human-gate suspension lives here.

#![forbid(unsafe_code)]

pub mod interfaces;
pub mod runtime;
pub mod types;

pub use interfaces::{
    sync_handler, BoxFuture, Checkpointer, Clock, ConditionFn, ConditionRegistry, EventBus,
    EventObserver, FallibleConditionFn, InMemoryCheckpointer, InMemoryConditionRegistry,
    InMemoryEventBus, InMemoryNodeRegistry, Interrupt, NodeHandler, NodeOutput, NodeRegistry,
    RecordedClock, RecordingClock, SystemClock,
};
pub use runtime::{CancelCheck, GraphRuntime, RuntimeError, APPROVED_TOOLS_CHANNEL};
pub use types::{Checkpoint, CheckpointId, RunEvent};
