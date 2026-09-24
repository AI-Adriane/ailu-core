//! Rust port of `@ailu/callbacks`.
//!
//! Provides the [`CallbackEvent`] lifecycle vocabulary, the [`CallbackHandler`]
//! and [`CallbackManager`] seams, and the default in-process
//! [`InMemoryCallbackManager`]. Handlers observe events emitted while a graph,
//! agent, tool, or LLM call runs; the manager fans each event out to every
//! registered handler and isolates handler failures from one another.

#![forbid(unsafe_code)]

mod error;
mod handlers;
mod interfaces;
mod manager;
mod types;

pub use error::CallbackError;
pub use handlers::{ConsoleCallbackHandler, MetricsCallbackHandler, NullCallbackHandler};
pub use interfaces::{CallbackHandler, CallbackManager};
pub use manager::InMemoryCallbackManager;
pub use types::{CallbackEvent, CallbackEventBase};
