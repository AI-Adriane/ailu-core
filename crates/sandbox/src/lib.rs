//! Ailu governed sandbox port — contract types.
//!
//! The engine never launches a process itself. This crate defines the *boundary*: the
//! identity and idempotency types, the typed limits and request bounds, the typed errors with
//! stable codes, and the pure validation of requests against deployment-set bounds. Backends
//! that actually execute code live outside the engine and are reached through a trait added
//! in a later step.
//!
//! Design rules enforced by the types themselves:
//!
//! - [`IdempotencyKey`] and [`SandboxHandle`] can only be built inside this crate, so a
//!   backend can never forge one.
//! - Request types reject unknown fields, so there is no way to smuggle a volume mount or a
//!   tenant identity into a request.
//! - Paths are [`LogicalPath`]s, validated with the same rule as the virtual filesystem.
//! - Everything is fail-closed: a doubtful request is a typed refusal.

#![forbid(unsafe_code)]

pub mod ids;
pub mod path;

pub use ids::{IdempotencyKey, SandboxContext, SandboxHandle, SandboxId, SandboxStep, TenantId};
pub use path::{InvalidPath, LogicalPath};
