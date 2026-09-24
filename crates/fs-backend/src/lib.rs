//! Ailu governed virtual filesystem seam (ADR 0024) — phase 2a.
//!
//! An agent-operable filesystem (`read`/`write`/`edit`/`delete`/`rename`/`ls`/
//! `glob`/`grep`) for context offloading, **governed by construction**: every write
//! is versioned + attributable (free from the artifact store), permissioned by a
//! per-path policy (fail-closed read-only by default), and — from phase 2c — gated
//! through the existing approval path. The shell/`execute` primitive is explicitly
//! NOT here: it is a separate external, always-gated seam (security hard rule).
//!
//! This crate is the framework-agnostic engine surface: the [`FilesystemBackend`]
//! trait, the default [`ArtifactFsBackend`] over [`ailu_artifact_store`], the
//! [`PathPolicy`] resolver, path normalization, and the wire types. The
//! agent-callable tools live in `ailu-agents-core` (`fs_tools`); the per-run
//! wiring into the runtime/bridge is a later phase.

#![forbid(unsafe_code)]

pub mod artifact_backend;
pub mod backend;
pub mod edits;
#[cfg(feature = "http")]
pub mod http_backend;
pub mod path;
pub mod policy;
pub mod types;

// Re-export the artifact-store types that surface in the fs wire (media type +
// version + ref) so downstream crates (e.g. the fs tools) need not depend on
// `ailu-artifact-store` directly.
pub use ailu_artifact_store::{ArtifactMediaType, ArtifactRef, ArtifactVersion};
pub use artifact_backend::ArtifactFsBackend;
pub use backend::{FilesystemBackend, NoopFilesystemBackend};
pub use edits::apply_edits;
#[cfg(feature = "http")]
pub use http_backend::HttpFilesystemBackend;
pub use path::normalize_path;
pub use policy::{glob_match, PathPolicy, PathRule, StaticPathPolicy};
pub use types::{EditOp, FileContent, FileEntry, FsError, FsPermVerb, FsWriteCtx, GrepMatch};
