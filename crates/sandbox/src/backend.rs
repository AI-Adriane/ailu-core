//! The execution boundary: what a backend implements.

use crate::cancel::CancelToken;
use crate::collected::RawCollected;
use crate::error::SandboxError;
use crate::ids::{IdempotencyKey, RawCreated, SandboxContext, SandboxHandle};
use crate::outcome::ExecOutcome;
use crate::request::{CollectRequest, CreateRequest, ExecRequest};
use async_trait::async_trait;

/// A backend executes; it does not decide policy. Everything it receives has already been
/// validated, bounded and (for `exec`) approved by the guard, which is the only code able to
/// build a [`SandboxHandle`] or an [`IdempotencyKey`].
///
/// Every operation is addressed with the same key for the same call: the same key with the
/// same request returns the result already obtained without running again, and `destroy` is
/// idempotent. `create` only reserves; a backend may build the execution resource at `exec`.
/// A backend signs nothing and approves nothing.
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// Reserve a sandbox. The guard wraps the result into a handle carrying the tenant of
    /// `ctx`.
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        cancel: &CancelToken,
    ) -> Result<RawCreated, SandboxError>;

    /// Run one program. A limit exceeded while running is
    /// `Err(LimitExceeded { kind, partial })`; output past its bound is truncated and marked
    /// in the `Ok` outcome.
    async fn exec(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: ExecRequest,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError>;

    /// Hand over the requested outputs, bounded, with the secret values the backend resolved.
    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError>;

    /// Release the sandbox. Idempotent, and not cancellable.
    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError>;
}
