//! The conformance suite: the contract of the sandbox port, as cases that run against any
//! backend.
//!
//! A backend is conformant when [`run_conformance`] reports no failure and at least one pass
//! ([`ConformanceReport::is_conformant`]), and its report is complete when, besides, every case
//! that was not applicable is one the caller accepted
//! ([`ConformanceReport::is_complete`]): a production backend lists those in its report. The
//! suite drives it through the guard ([`crate::GovernedSandbox`]) and, for the obligations only a backend can
//! show, directly. It starts no process and needs no runtime: it is plain futures, so any
//! executor can run it.
//!
//! Available with the Cargo feature `conformance`, which is off by default and does not
//! imply `sim`. With both `conformance` and `sim` the crate also provides `SimTarget`, the
//! simulated backend as a target, which must pass every case that applies to it.
//!
//! # What a target provides
//!
//! A [`ConformanceTarget`] makes a fresh instance of the backend for each case, with the clock
//! it shares with the guard, and answers [`ConformanceTarget::probe`]: the command that
//! provokes each behavior (a loop, a memory hog, a connection to a given destination, the
//! reading of a secret, an outgoing link). A target that cannot provoke a behavior answers
//! `None`, and the cases that need it are reported [`CaseOutcome::NotApplicable`], never as
//! passed. The same goes for the optional [`Hooks`]: inspecting residues after a `ttl`,
//! injecting a host crash, reading the idempotency marker, moving the clock.
//!
//! # The three classes of cases
//!
//! - [`ConformanceClass::Guard`]: properties of the guard (policy, approval, idempotence,
//!   registry, redaction, events). The suite makes the backend misbehave by putting a tap
//!   between the two, so these hold whatever the backend and need no hook.
//! - [`ConformanceClass::SimulatorOnly`]: a host crash at a precise point, or days of clock in
//!   one step. Reported as not applicable for a target whose hooks cannot do it.
//! - [`ConformanceClass::RealBackend`]: the obligations of the backend itself: isolation,
//!   limits, network, residues and sweeps, durable idempotence.
//!
//! A few cases only measure (an allowed host that relays traffic is the accepted limit of an
//! allow list): they end as [`CaseOutcome::Measured`], neither a pass nor a failure.
//!
//! # Deadlines
//!
//! The suite starts no timer and a backend that never answers hangs it. Bound each case with
//! [`run_conformance_with`] and the timeout of your own executor, or run the whole suite under
//! an external timeout.
//!
//! # What the suite does not prove
//!
//! A backend that lies consistently about the content it produces is out of reach, and so is
//! the strength of a real isolation: the suite shows that the attacks it can provoke fail in a
//! typed and journaled way. The report holds ids, classes, counts and reasons made of codes
//! and fixed words; it carries no content of a run.
//!
//! The execution proof is covered by the `proof` group, for a target that gives a
//! [`ProofKit`]: a signer and the keys that verify it. It checks the signer's contract (idempotent
//! by key, one ordered chain per run, a refusal for what is malformed), the guard's (no signed
//! proof, no result, on a first collect and on a replay) and the verifier's (pinned keys, a
//! signed `kind`, no mixed chain). Like every other case, it cannot tell that a backend lied
//! consistently about the content it produced: the proof attests what was declared and what the
//! guard recomputed.

mod cases;
#[cfg(all(test, feature = "sim"))]
mod guard_mutants;
mod report;
mod runner;
mod support;
mod target;

#[cfg(feature = "sim")]
mod sim_target;

pub use report::{Case, CaseOutcome, CaseResult, ConformanceClass, ConformanceReport, Group};
pub use runner::{
    catalogue, run_cases, run_cases_with, run_conformance, run_conformance_with, CaseFuture,
};
#[cfg(feature = "sim")]
pub use sim_target::SimTarget;
pub use target::{
    ConformanceTarget, Hooks, Instance, MarkerState, NetTarget, NoHooks, ProbeKind, ProbeProgram,
    ProofKit,
};

use crate::limits::Limits;

const MIB: u64 = 1024 * 1024;

/// The limits the resource probes run under: low enough to be reached quickly, and inside the
/// default ceiling. A probe of [`ProbeKind::WallTime`], [`ProbeKind::Memory`],
/// [`ProbeKind::Processes`], [`ProbeKind::Disk`] or [`ProbeKind::HugeOutput`] must exceed the
/// matching field:
///
/// | field | value |
/// | --- | --- |
/// | wall time | 2 seconds |
/// | memory | 64 MiB |
/// | processes | 8 |
/// | ephemeral disk | 8 MiB |
/// | output, per stream | 64 KiB |
pub fn probe_limits() -> Limits {
    Limits {
        cpu_millicores: 250,
        memory_bytes: 64 * MIB,
        wall_time_ms: 2_000,
        output_bytes: 64 * 1024,
        max_processes: 8,
        ephemeral_disk_bytes: 8 * MIB,
    }
}
