//! A reference backend that runs no program at all.
//!
//! The engine never launches a process, so its reference backend is **entirely in memory and
//! deterministic**: "programs" are registered Rust functions that run over a virtual
//! filesystem, time is a virtual clock that tests move by hand, and the network is a table
//! of simulated hosts. It proves the *semantics* of the contract (states, idempotence,
//! policy, errors); it does not prove the isolation of any production backend.
//!
//! Available with the Cargo feature `sim`, which is off by default.

mod clock;
mod fault;
mod fs;
mod net;
mod output;

pub use clock::SimClock;
pub use fault::{FaultNotApplicable, SimFault};
pub use fs::{SimFs, SimFsError};
pub use net::{
    classify_address, AddrClass, NetDenied, NetStats, SimConnection, SimHost, SimNetwork,
    MAX_REDIRECTS,
};
pub use output::{SimOutput, SimStream, SimUsage, MAX_STREAM_BYTES};
