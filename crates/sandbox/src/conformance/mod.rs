//! The conformance suite: the contract of the sandbox port, as cases that run against any
//! backend.
//!
//! A backend is conformant when [`run_conformance`] reports no failure. The suite drives it
//! through the guard ([`crate::GovernedSandbox`]) and, for the obligations only a backend can
//! show, directly. It starts no process and needs no runtime: it is plain futures, so any
//! executor can run it.
//!
//! Available with the Cargo feature `conformance`, which is off by default and does not
//! imply `sim`.
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
//! # What the suite does not prove
//!
//! A backend that lies consistently about the content it produces is out of reach, and so is
//! the strength of a real isolation: the suite shows that the attacks it can provoke fail in a
//! typed and journaled way. The report holds ids, classes, counts and reasons made of codes
//! and fixed words; it carries no content of a run.
//!
//! The execution proof and its signer are not part of the suite yet.

mod cases;
mod report;
mod runner;
mod support;
mod target;

pub use report::{Case, CaseOutcome, CaseResult, ConformanceClass, ConformanceReport, Group};
pub use runner::{catalogue, run_cases, run_conformance};
pub use target::{
    ConformanceTarget, Hooks, Instance, MarkerState, NetTarget, NoHooks, ProbeKind, ProbeProgram,
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
