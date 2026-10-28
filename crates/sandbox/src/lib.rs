//! Ailu governed sandbox port.
//!
//! The engine never launches a process itself. This crate defines the *boundary*: the
//! identity and idempotency types, the typed limits and request bounds, the typed errors with
//! stable codes, the pure validation of requests against deployment-set bounds, the
//! [`SandboxBackend`] trait that an execution backend implements, and the
//! [`GovernedSandbox`] guard that stands in front of it. Backends that actually execute code
//! live outside the engine; the guard is the only way to reach one.
//!
//! The guard applies the deployment [`SandboxPolicy`], requires an approval, verified by a [`GrantVerifier`] and bound to
//! the exact call before anything is created or run, enforces the lifecycle and per-tenant
//! admission, and records one event per transition in its own [`SandboxJournal`].
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
pub mod guard;
pub mod ids;
pub mod journal;
pub mod limits;
pub mod outcome;
pub mod path;
pub mod policy;
pub mod request;
pub mod secret;
pub mod validate;

pub use approval::{
    ApprovalClaim, ApprovalRequest, GrantError, GrantVerifier, InMemoryGrantVerifier,
    VerifiedGrant, EXEC_CALL_NAME,
};
pub use backend::SandboxBackend;
pub use cancel::CancelToken;
pub use clock::SandboxClock;
pub use collected::{Collected, CollectedFile, RawCollected, RawFile, ResolvedSecrets};
pub use error::{
    ApprovalSubject, BoundKind, LimitField, LimitKind, PolicyReason, QuotaKind, SandboxError,
    SandboxState,
};
pub use guard::{GovernedSandbox, SandboxSession, SessionFuture};
pub use ids::{
    IdempotencyKey, IsolationDomain, RawCreated, SandboxContext, SandboxHandle, SandboxId,
    SandboxStep, TenantId,
};
pub use journal::{
    Appended, InMemoryJournal, JournalError, SandboxEvent, SandboxEventKind, SandboxJournal,
};
pub use limits::{Limits, RequestBounds};
pub use outcome::{DeclaredExit, ExecOutcome, Truncation};
pub use path::{InvalidPath, LogicalPath};
pub use policy::{
    AttemptWindow, PolicyConfigError, SandboxPolicy, TenantQuota, DEFAULT_RESULT_RETENTION_MS,
    MAX_RESULT_RETENTION_MS,
};
pub use request::{
    CollectRequest, CreateRequest, EnvGrant, ExecRequest, HostRule, InputRef, NetworkPolicy,
    SecretRef,
};
pub use secret::ResolvedSecret;
pub use validate::{validate_collect, validate_context, validate_create, validate_exec};
