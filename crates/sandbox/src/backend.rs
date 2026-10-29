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
///
/// # What a backend must do
///
/// The guard cannot see inside a backend, so these are obligations of the backend itself.
/// The conformance suite (the `conformance` Cargo feature of this crate) proves them against any
/// backend that describes itself as a `ConformanceTarget`.
///
/// - **Deduplicate on the key.** The guard asks again with the same key after a `Cancelled`,
///   after an `Unavailable { retriable: true }` and after a `Busy`, and after a restart of the
///   host it replays whole nodes. A backend that ran the program on the first call must not
///   run it a second time for the same key and the same request, and must answer the same key
///   with another request by `IdempotencyConflict`, in every state.
/// - **Keep the answers durably, per tenant.** For a declared retention (at least the resume
///   window of a run) the backend keeps, in storage isolated per tenant, at least
///   `(key -> request digest, result)`. The records are serializable and bounded. A backend
///   without durable storage may only serve tests.
/// - **Write an execution marker before running.** If a marker is present, the result is
///   absent and no execution resource is left, the answer is `Unavailable { retriable: false }`
///   and the program is never run again.
/// - **One isolation domain per sandbox, never shared between tenants**, not even after
///   `destroy`. The domain returned by `create` is the one the proof will name.
/// - **Answer `NotFound`** for a handle or key that belongs to another tenant, with no
///   disclosure. Run at most one `exec` and one `collect` per sandbox: a second `collect`
///   under another key is an `InvalidState` (the sandbox is `Collected`), while the same
///   `collect` under the same key returns the recorded result.
/// - **Bound everything.** Output past `output_bytes` is truncated and marked, never
///   buffered without bound; a limit exceeded while running kills the program and returns
///   `LimitExceeded` with the partial outcome.
/// - **Consult the [`CancelToken`]** during `create`, `exec` and `collect`, before any effect
///   it could still avoid. `destroy` is not cancellable.
/// - **Sweep on `ttl`.** A resource still present after its `ttl` is a conformance failure.
/// - **Honour the attempt deadline.** `create`, `exec` and `collect` receive
///   `attempt_deadline_ms`: the last instant at which the host may ask again for this attempt (the
///   start of the run plus the retention recorded when it started, see `AttemptWindow`). A
///   backend MUST keep what it needs to deduplicate the key at least until that instant, plus a
///   margin of its own, and MUST NOT shorten that period when its own configuration changes
///   afterwards. It MUST refuse (fail closed, with any error: the guard answers `backend_error`
///   and never relays a backend's policy code) a deadline later than `now + max_retention()`
///   when it has to seal the key, because it could not keep that promise; and it MUST refuse a
///   repeated key whose deadline is later than the one it sealed, because the state that would
///   answer the repeat may already be gone.
/// - **Do not assume input artifacts are checked.** Input artifacts are NOT yet checked for run
///   ownership nor bounded by `max_input_bytes`; implementers MUST NOT assume it until the
///   artifact-store lots; the guard will enforce it. Tracked in #388.
/// - **Enforce the network policy** (deny by default, DNS included; off-list hosts and
///   redirects refused; private, loopback, link-local and metadata addresses refused, in IPv4,
///   IPv6 and IPv4-mapped IPv6 forms; DNS rebinding refused) and **hand back the resolved
///   secret values** to the guard in `RawCollected`, one per grant, for redaction only.
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// The longest time, in milliseconds, the backend keeps what it needs to answer a repeated
    /// key without running again. The guard never accepts an attempt older than this, and the
    /// host must not configure a result retention above it (the guard refuses to start with
    /// one). It is a promise about the backend's own deduplication state, and it must not
    /// change while the backend runs.
    fn max_retention(&self) -> u64;

    /// Reserve a sandbox. The guard wraps the result into a handle carrying the tenant of
    /// `ctx`.
    async fn create(
        &self,
        ctx: &SandboxContext,
        key: &IdempotencyKey,
        req: CreateRequest,
        attempt_deadline_ms: u64,
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
        attempt_deadline_ms: u64,
        cancel: &CancelToken,
    ) -> Result<ExecOutcome, SandboxError>;

    /// Hand over the requested outputs, bounded, with the secret values the backend resolved.
    async fn collect(
        &self,
        h: &SandboxHandle,
        key: &IdempotencyKey,
        req: CollectRequest,
        attempt_deadline_ms: u64,
        cancel: &CancelToken,
    ) -> Result<RawCollected, SandboxError>;

    /// Release the sandbox. Idempotent, and not cancellable.
    async fn destroy(&self, h: &SandboxHandle, key: &IdempotencyKey) -> Result<(), SandboxError>;
}
