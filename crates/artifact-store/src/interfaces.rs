//! The [`ArtifactStore`] seam — the Rust port of `@ailu/artifact-store`'s
//! `interfaces.ts`. Mirrors the TS async API one-to-one.

use ailu_graph_core::RunId;
use async_trait::async_trait;

use crate::types::{Artifact, ArtifactId, ArtifactVersion, ArtifactWriteInput};

/// Persistence seam for versioned artifacts.
///
/// The async signatures mirror the TS `Promise`-returning methods so any
/// backing store (in-memory, Postgres, …) can implement the same contract.
/// Lookups that may miss return `Option` — the Rust equivalent of the TS
/// `Artifact | undefined`.
///
/// Implementations must be `Send + Sync` so a single store can be shared across
/// the runtime's tasks.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Persist a new artifact. The first write for a given `runId`/`name`
    /// produces version `1`; each subsequent write of the same pair increments
    /// the version and reuses the same [`ArtifactId`]. Returns the stored
    /// artifact, including its assigned `id`, `version`, and `createdAt`.
    async fn write(&self, input: ArtifactWriteInput) -> Artifact;

    /// Read the latest version of the artifact with the given id, if any.
    async fn read(&self, id: &ArtifactId) -> Option<Artifact>;

    /// Read an exact version of the artifact with the given id, if it exists.
    async fn read_version(&self, id: &ArtifactId, version: ArtifactVersion) -> Option<Artifact>;

    /// List every artifact version produced during the given run (all names,
    /// all versions).
    async fn list_by_run(&self, run_id: &RunId) -> Vec<Artifact>;

    /// List every version of a single artifact id, in ascending version order.
    async fn list_versions(&self, id: &ArtifactId) -> Vec<Artifact>;
}
