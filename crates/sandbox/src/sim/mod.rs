//! A reference backend that runs no program at all.
//!
//! The engine never launches a process, so its reference backend is **entirely in memory and
//! deterministic**: "programs" are registered Rust functions that run over a virtual
//! filesystem, time is a virtual clock that tests move by hand, and the network is a table
//! of simulated hosts. It proves the *semantics* of the contract (states, idempotence,
//! policy, errors); it does not prove the isolation of any production backend.
//!
//! Available with the Cargo feature `sim`, which is off by default.
//!
//! What to keep in mind when reading a result:
//!
//! - The network is a table of **exact names**. The simulator analyses no host string (no
//!   IP literal, no wildcard, no odd spelling): the barrier against those is the request
//!   validation of the guard, which refuses them before any backend is reached.
//! - A secret that cannot be resolved (unknown, or another tenant's) is `NotFound` at the
//!   backend. The guard turns every `NotFound` a backend returns into a generic backend
//!   error, so a caller never learns which of the two it was.
//! - A tenant whose durable records fill up is answered `QuotaExceeded`; the guard likewise
//!   makes it generic.
//! - An outgoing symbolic link among the collected outputs is modelled ([`SimFs::symlink`])
//!   and refused at `collect` with `InvalidPath`.
//! - It proves the semantics of the contract, not the isolation of any real backend.

// The simulator must not run a process, touch a file, read the host clock or environment,
// open a socket or start a thread: see `clippy.toml` at the root of the crate.
#![deny(clippy::disallowed_types, clippy::disallowed_methods)]

mod backend;
mod clock;
mod fault;
mod fs;
mod net;
mod output;
mod program;
// Shared by the network tests.
#[cfg(test)]
pub(crate) mod reference;
mod secrets;

pub use backend::{SimBackend, SimConfig, SweepReport};
pub use clock::SimClock;
pub use fault::{FaultNotApplicable, SimFault};
pub use fs::{SimFs, SimFsError};
pub use net::{
    classify_address, AddrClass, NetDenied, NetStats, SimConnection, SimHost, SimNetwork,
    MAX_REDIRECTS,
};
pub use output::{SimOutput, SimStream, SimUsage, MAX_STREAM_BYTES};
pub use program::{SimInvocation, SimProgram, SimResult};
pub use secrets::SimSecrets;

// Tests start threads on purpose, to overlap calls.
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod backend_tests;
#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod test_util;
