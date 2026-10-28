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

pub mod approval;
pub mod backend;
pub mod cancel;
pub mod clock;
pub mod collected;
mod digest;
pub mod error;
pub mod ids;
pub mod limits;
pub mod outcome;
pub mod path;
pub mod policy;
pub mod request;
pub mod secret;
pub mod validate;

pub use approval::{ApprovalGrant, ApprovalRequest, EXEC_CALL_NAME};
pub use backend::SandboxBackend;
pub use cancel::CancelToken;
pub use clock::SandboxClock;
pub use collected::{Collected, CollectedFile, RawCollected, RawFile, ResolvedSecrets};
pub use error::{
    ApprovalSubject, BoundKind, LimitField, LimitKind, PolicyReason, QuotaKind, SandboxError,
    SandboxState,
};
pub use ids::{
    IdempotencyKey, IsolationDomain, RawCreated, SandboxContext, SandboxHandle, SandboxId,
    SandboxStep, TenantId,
};
pub use limits::{Limits, RequestBounds};
pub use outcome::{DeclaredExit, ExecOutcome, Truncation};
pub use path::{InvalidPath, LogicalPath};
pub use policy::{
    PolicyConfigError, SandboxPolicy, TenantQuota, DEFAULT_RESULT_RETENTION_MS,
    MAX_RESULT_RETENTION_MS,
};
pub use request::{
    CollectRequest, CreateRequest, EnvGrant, ExecRequest, HostRule, InputRef, NetworkPolicy,
    SecretRef,
};
pub use secret::ResolvedSecret;
pub use validate::{validate_collect, validate_context, validate_create, validate_exec};
