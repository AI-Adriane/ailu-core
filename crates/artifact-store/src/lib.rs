//! Ailu artifact-store (Rust).
//!
//! Versioned artifact storage — the Rust port of `@ailu/artifact-store`.
//! Provides the [`ArtifactStore`] seam, the [`Artifact`]/[`ArtifactRef`] data
//! model, and an in-memory implementation.
//!
//! Versioning rule (faithful to the TS): an artifact's id is derived from its
//! `runId:name`; the first write for that pair is version `1`, and each later
//! write of the same pair appends an incremented version while reusing the id.
//!
//! ## Deferred: `PgArtifactStore`
//!
//! The Postgres-backed store (TS `PgArtifactStore`) is intentionally **not**
//! ported here. It is deferred to a later wave alongside the rest of the
//! database layer, so this crate carries no `db` dependency. When added, it
//! must implement the same [`ArtifactStore`] trait.

#![forbid(unsafe_code)]

pub mod in_memory_artifact_store;
pub mod interfaces;
pub mod types;

pub use in_memory_artifact_store::InMemoryArtifactStore;
pub use interfaces::ArtifactStore;
pub use types::{
    Artifact, ArtifactId, ArtifactMediaType, ArtifactRef, ArtifactVersion, ArtifactWriteInput,
};
