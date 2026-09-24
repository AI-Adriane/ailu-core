//! The [`FilesystemBackend`] seam (ADR 0024): the pluggable storage contract the fs
//! tools call. Default impl is [`crate::ArtifactFsBackend`] over the versioned
//! artifact store; an external backend rides `HttpFilesystemBackend` (later phase),
//! and [`NoopFilesystemBackend`] disables the fs.

use ailu_artifact_store::{ArtifactMediaType, ArtifactRef, ArtifactVersion};
use async_trait::async_trait;

use crate::types::{EditOp, FileContent, FileEntry, FsError, FsWriteCtx, GrepMatch};

/// Storage seam for the governed virtual filesystem. Run-scoped: an instance is
/// bound to a single run, so `read`/`ls`/`glob`/`grep` need no context; mutating
/// ops carry an [`FsWriteCtx`] for attribution. Implementations are policy-agnostic
/// — permission enforcement lives in the fs tools (and, from phase 2c, a backend
/// re-check for guarded paths).
#[async_trait]
pub trait FilesystemBackend: Send + Sync {
    /// Read a file (latest version, or an exact `version`). A tombstoned (deleted)
    /// file reads as [`FsError::NotFound`].
    async fn read(
        &self,
        path: &str,
        version: Option<ArtifactVersion>,
    ) -> Result<FileContent, FsError>;

    /// Write `content` to `path`, producing a new version. Returns the new ref.
    async fn write(
        &self,
        path: &str,
        content: String,
        media_type: ArtifactMediaType,
        ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError>;

    /// Apply line patches to the latest version of `path`, writing the result as a
    /// new version (the pre-edit content is preserved as the prior version).
    async fn edit(
        &self,
        path: &str,
        patches: Vec<EditOp>,
        ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError>;

    /// Tombstone `path` (a new version with `metadata.deleted = true`). History stays
    /// queryable for audit; the file then reads as not-found and is excluded from listings.
    async fn delete(&self, path: &str, ctx: &FsWriteCtx) -> Result<(), FsError>;

    /// Copy `from`'s latest content to `to` (a new artifact) and tombstone `from`.
    /// Returns the ref of the new `to` artifact.
    async fn rename(&self, from: &str, to: &str, ctx: &FsWriteCtx) -> Result<ArtifactRef, FsError>;

    /// List the immediate children of a directory `prefix` (`""` = root). Synthetic
    /// directories appear for any prefix under which a file lives.
    async fn ls(&self, prefix: &str) -> Result<Vec<FileEntry>, FsError>;

    /// Return the paths matching a glob (`*` within a segment, `**` across segments).
    async fn glob(&self, pattern: &str) -> Result<Vec<String>, FsError>;

    /// Substring-search file contents. `paths` empty = search every (live) file.
    async fn grep(&self, pattern: &str, paths: Vec<String>) -> Result<Vec<GrepMatch>, FsError>;
}

/// A backend that supports nothing — for deployments that explicitly disable the fs.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopFilesystemBackend;

#[async_trait]
impl FilesystemBackend for NoopFilesystemBackend {
    async fn read(&self, _p: &str, _v: Option<ArtifactVersion>) -> Result<FileContent, FsError> {
        Err(FsError::NotSupported)
    }
    async fn write(
        &self,
        _p: &str,
        _c: String,
        _m: ArtifactMediaType,
        _ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError> {
        Err(FsError::NotSupported)
    }
    async fn edit(
        &self,
        _p: &str,
        _e: Vec<EditOp>,
        _ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError> {
        Err(FsError::NotSupported)
    }
    async fn delete(&self, _p: &str, _ctx: &FsWriteCtx) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }
    async fn rename(&self, _f: &str, _t: &str, _ctx: &FsWriteCtx) -> Result<ArtifactRef, FsError> {
        Err(FsError::NotSupported)
    }
    async fn ls(&self, _prefix: &str) -> Result<Vec<FileEntry>, FsError> {
        Err(FsError::NotSupported)
    }
    async fn glob(&self, _pattern: &str) -> Result<Vec<String>, FsError> {
        Err(FsError::NotSupported)
    }
    async fn grep(&self, _pattern: &str, _paths: Vec<String>) -> Result<Vec<GrepMatch>, FsError> {
        Err(FsError::NotSupported)
    }
}
