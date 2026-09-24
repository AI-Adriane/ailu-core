//! Ailu approval-engine (Rust).
//!
//! Human-approval gates with tamper-evident Ed25519 attestation — the Rust port of
//! `@ailu-ai/approval-engine`. An agent never approves its own request; resolved
//! decisions can be signed and chained for a verifiable audit trail.

#![forbid(unsafe_code)]

pub mod attestation;
pub mod engine;
pub mod error;
pub mod types;

pub use attestation::{
    canonical_json, hash_view, verify_attestation, verify_chain, AttestationRecord,
    AttestationView, Ed25519Attestor,
};
pub use engine::{ApprovalEngine, InMemoryApprovalEngine, RequestApprovalParams};
pub use error::ApprovalError;
pub use types::{ApprovalId, ApprovalRequest, ApprovalStatus};
