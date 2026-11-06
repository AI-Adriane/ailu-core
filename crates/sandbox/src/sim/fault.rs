//! Faults a test can inject, or a simulated program can raise.

use crate::error::{LimitKind, SandboxError};
use crate::ids::SandboxStep;

/// Something that goes wrong in the simulated backend.
///
/// A fault reaches the backend in one of two ways. A **script** queues it for the next call
/// of a step ([`crate::sim::SimBackend::inject`]); it is applied before the call has any
/// effect, except where noted. A **program** returns it as the `Err` of its result, which
/// models the fault happening while the program runs. A fault that does not make sense for
/// the step it reaches is refused when scripted, and answered as a generic backend error when
/// a program raises it.
///
/// `Debug` shows the kind of fault and, for an error, its stable code: never the text it
/// carries.
#[derive(Clone, PartialEq, Eq)]
pub enum SimFault {
    /// `exec`: the program is killed because it went over a limit (an infinite loop, an
    /// unbounded allocation, a process explosion, a disk fill). The answer is
    /// `LimitExceeded` with the partial outcome, and the output produced so far stays
    /// collectable.
    Limit(LimitKind),
    /// `exec`: the program writes an enormous standard output (a virtual terabyte). The
    /// simulator keeps only what the output bound allows and marks the outcome truncated.
    HugeOutput,
    /// Any step: the backend answers this error. Scripted, it is answered before the call
    /// has any effect (a sandbox that receives an `IsolationViolation` is poisoned). Raised by
    /// a program during `exec`, a `Busy` or `Unavailable` leaves the outcome unknown (the
    /// marker stays, no result is kept) and any other error is kept as the definitive result.
    Error(SandboxError),
    /// `exec`: the backend answers a `Backend` error whose free text repeats the arguments
    /// and the standard input of the request, as a careless backend might.
    EchoRequest,
    /// `exec`: the host dies after the program ran and before its result was stored. The
    /// caller is told `Unavailable { retriable: true }`; asking again finds the marker without
    /// a result and gets `Unavailable { retriable: false }`, and the program is not run again.
    CrashAfterMarker,
    /// `create`: the backend returns an isolation domain already held by another tenant, when
    /// there is one.
    SharedDomain,
    /// `collect`: the digests declared do not match the bytes delivered.
    ForgedOutput,
    /// `collect`: one resolved secret fewer than there are grants is handed back.
    FewerSecrets,
}

impl SimFault {
    /// Shorthand for `Error(Unavailable { retriable })`.
    pub fn unavailable(retriable: bool) -> Self {
        Self::Error(SandboxError::Unavailable { retriable })
    }

    /// Shorthand for `Error(Busy { retry_after_ms })`.
    pub fn busy(retry_after_ms: Option<u64>) -> Self {
        Self::Error(SandboxError::Busy { retry_after_ms })
    }

    /// Shorthand for `Error(IsolationViolation { signal })`.
    pub fn violation(signal: impl Into<String>) -> Self {
        Self::Error(SandboxError::IsolationViolation {
            signal: signal.into(),
        })
    }

    /// Shorthand for `Error(Backend(message))`.
    pub fn backend(message: impl Into<String>) -> Self {
        Self::Error(SandboxError::Backend(message.into()))
    }

    /// Whether this fault means something for `step`.
    pub fn applies_to(&self, step: SandboxStep) -> bool {
        match self {
            SimFault::Error(_) => true,
            SimFault::Limit(_)
            | SimFault::HugeOutput
            | SimFault::EchoRequest
            | SimFault::CrashAfterMarker => step == SandboxStep::Exec,
            SimFault::SharedDomain => step == SandboxStep::Create,
            SimFault::ForgedOutput | SimFault::FewerSecrets => step == SandboxStep::Collect,
        }
    }
}

impl std::fmt::Debug for SimFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimFault::Limit(kind) => f.debug_tuple("Limit").field(kind).finish(),
            SimFault::HugeOutput => f.write_str("HugeOutput"),
            SimFault::Error(error) => f.debug_tuple("Error").field(&error.full_code()).finish(),
            SimFault::EchoRequest => f.write_str("EchoRequest"),
            SimFault::CrashAfterMarker => f.write_str("CrashAfterMarker"),
            SimFault::SharedDomain => f.write_str("SharedDomain"),
            SimFault::ForgedOutput => f.write_str("ForgedOutput"),
            SimFault::FewerSecrets => f.write_str("FewerSecrets"),
        }
    }
}

/// A scripted fault that cannot happen at the step it was queued for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("this fault does not apply to that step")]
pub struct FaultNotApplicable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_apply_to_the_steps_they_describe() {
        use SandboxStep::*;
        for step in [Create, Exec, Collect, Destroy] {
            assert!(SimFault::unavailable(true).applies_to(step));
        }
        assert!(SimFault::Limit(LimitKind::Disk).applies_to(Exec));
        assert!(!SimFault::Limit(LimitKind::Disk).applies_to(Collect));
        assert!(SimFault::SharedDomain.applies_to(Create));
        assert!(!SimFault::SharedDomain.applies_to(Exec));
        assert!(SimFault::ForgedOutput.applies_to(Collect));
        assert!(!SimFault::FewerSecrets.applies_to(Destroy));
        assert!(SimFault::CrashAfterMarker.applies_to(Exec));
        assert!(!SimFault::EchoRequest.applies_to(Create));
    }

    #[test]
    fn debug_never_shows_the_text_of_an_error() {
        let shown = format!("{:?}", SimFault::backend("token=s3cr3t"));
        assert!(!shown.contains("s3cr3t"), "{shown}");
        assert!(shown.contains("backend_error"), "{shown}");
        let shown = format!("{:?}", SimFault::violation("s3cr3t-signal"));
        assert!(!shown.contains("s3cr3t"), "{shown}");
    }
}
