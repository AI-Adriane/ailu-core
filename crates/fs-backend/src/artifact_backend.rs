//! [`ArtifactFsBackend`] — the DEFAULT governed fs backend (ADR 0024). Maps the
//! virtual filesystem onto the existing versioned [`ArtifactStore`] with NO new
//! store method and NO schema change: path = artifact `name`, write = new version,
//! delete/rename = tombstone (`metadata.deleted = true`). Directories are synthetic
//! views over the flat keyspace.

use std::collections::BTreeMap;
use std::sync::Arc;

use ailu_artifact_store::{
    Artifact, ArtifactId, ArtifactMediaType, ArtifactRef, ArtifactStore, ArtifactVersion,
    ArtifactWriteInput,
};
use ailu_graph_core::RunId;
use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::backend::FilesystemBackend;
use crate::edits::apply_edits;
use crate::path::normalize_path;
use crate::types::{EditOp, FileContent, FileEntry, FsError, FsWriteCtx, GrepMatch};

/// A run-scoped governed fs over an [`ArtifactStore`]. All paths become artifact
/// names under this run; tenant scoping is enforced at the control-plane boundary
/// (the engine store is run-scoped).
pub struct ArtifactFsBackend {
    store: Arc<dyn ArtifactStore>,
    run_id: RunId,
}

impl ArtifactFsBackend {
    pub fn new(store: Arc<dyn ArtifactStore>, run_id: RunId) -> Self {
        ArtifactFsBackend { store, run_id }
    }

    fn id_for(&self, name: &str) -> ArtifactId {
        ArtifactId::from(format!("{}:{}", self.run_id, name))
    }

    fn metadata(&self, ctx: &FsWriteCtx, deleted: bool) -> Option<Map<String, Value>> {
        let mut map = Map::new();
        if let Some(principal) = &ctx.principal {
            map.insert("principal".to_owned(), Value::String(principal.clone()));
        }
        if deleted {
            map.insert("deleted".to_owned(), Value::Bool(true));
        }
        if map.is_empty() {
            None
        } else {
            Some(map)
        }
    }

    async fn write_raw(
        &self,
        name: &str,
        content: String,
        media_type: ArtifactMediaType,
        ctx: &FsWriteCtx,
        deleted: bool,
    ) -> ArtifactRef {
        let artifact = self
            .store
            .write(ArtifactWriteInput {
                run_id: self.run_id.clone(),
                node_id: ctx.node_id.clone(),
                name: name.to_owned(),
                media_type,
                content: Value::String(content),
                metadata: self.metadata(ctx, deleted),
            })
            .await;
        artifact.as_ref()
    }

    /// Latest live (non-tombstoned) artifact for a name, if any.
    async fn latest_live(&self, name: &str) -> Option<Artifact> {
        let artifact = self.store.read(&self.id_for(name)).await?;
        if is_tombstoned(&artifact) {
            None
        } else {
            Some(artifact)
        }
    }

    /// Latest version per name across the whole run, tombstones excluded.
    async fn live_latest_set(&self) -> Vec<Artifact> {
        let all = self.store.list_by_run(&self.run_id).await;
        let mut latest: BTreeMap<String, Artifact> = BTreeMap::new();
        for artifact in all {
            match latest.get(&artifact.name) {
                Some(existing) if existing.version >= artifact.version => {}
                _ => {
                    latest.insert(artifact.name.clone(), artifact);
                }
            }
        }
        latest.into_values().filter(|a| !is_tombstoned(a)).collect()
    }
}

fn is_tombstoned(artifact: &Artifact) -> bool {
    artifact
        .metadata
        .as_ref()
        .and_then(|m| m.get("deleted"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn text_of(artifact: &Artifact) -> String {
    artifact
        .content
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| artifact.content.to_string())
}

fn to_file_content(artifact: &Artifact, path: &str) -> FileContent {
    FileContent {
        path: path.to_owned(),
        content: text_of(artifact),
        media_type: artifact.media_type,
        version: artifact.version,
        created_at: artifact.created_at.clone(),
    }
}

/// Normalize an `ls` prefix: root sentinels (`""`, `.`, `/`) → `""`; else a path.
fn dir_prefix(prefix: &str) -> Result<String, FsError> {
    match prefix.trim() {
        "" | "." | "/" => Ok(String::new()),
        other => normalize_path(other),
    }
}

#[async_trait]
impl FilesystemBackend for ArtifactFsBackend {
    async fn read(
        &self,
        path: &str,
        version: Option<ArtifactVersion>,
    ) -> Result<FileContent, FsError> {
        let name = normalize_path(path)?;
        let id = self.id_for(&name);
        let artifact = match version {
            Some(v) => self.store.read_version(&id, v).await,
            None => self.store.read(&id).await,
        };
        match artifact {
            Some(a) if !is_tombstoned(&a) => Ok(to_file_content(&a, &name)),
            _ => Err(FsError::NotFound { path: name }),
        }
    }

    async fn write(
        &self,
        path: &str,
        content: String,
        media_type: ArtifactMediaType,
        ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError> {
        let name = normalize_path(path)?;
        Ok(self.write_raw(&name, content, media_type, ctx, false).await)
    }

    async fn edit(
        &self,
        path: &str,
        patches: Vec<EditOp>,
        ctx: &FsWriteCtx,
    ) -> Result<ArtifactRef, FsError> {
        let name = normalize_path(path)?;
        let current = self
            .latest_live(&name)
            .await
            .ok_or_else(|| FsError::NotFound { path: name.clone() })?;
        let next = apply_edits(&text_of(&current), &patches)?;
        Ok(self
            .write_raw(&name, next, current.media_type, ctx, false)
            .await)
    }

    async fn delete(&self, path: &str, ctx: &FsWriteCtx) -> Result<(), FsError> {
        let name = normalize_path(path)?;
        let current = self
            .latest_live(&name)
            .await
            .ok_or_else(|| FsError::NotFound { path: name.clone() })?;
        // Tombstone = a new empty version flagged deleted; history stays for audit.
        self.write_raw(&name, String::new(), current.media_type, ctx, true)
            .await;
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str, ctx: &FsWriteCtx) -> Result<ArtifactRef, FsError> {
        let from_name = normalize_path(from)?;
        let to_name = normalize_path(to)?;
        let current = self
            .latest_live(&from_name)
            .await
            .ok_or_else(|| FsError::NotFound {
                path: from_name.clone(),
            })?;
        let new_ref = self
            .write_raw(&to_name, text_of(&current), current.media_type, ctx, false)
            .await;
        // Tombstone the source after the copy lands.
        self.write_raw(&from_name, String::new(), current.media_type, ctx, true)
            .await;
        Ok(new_ref)
    }

    async fn ls(&self, prefix: &str) -> Result<Vec<FileEntry>, FsError> {
        let prefix = dir_prefix(prefix)?;
        let scope = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut dirs: BTreeMap<String, ()> = BTreeMap::new();
        let mut files: Vec<FileEntry> = Vec::new();
        for artifact in self.live_latest_set().await {
            let rel = if scope.is_empty() {
                Some(artifact.name.as_str())
            } else {
                artifact.name.strip_prefix(&scope)
            };
            let Some(rel) = rel else { continue };
            if rel.is_empty() {
                continue;
            }
            match rel.split_once('/') {
                Some((first, _)) => {
                    let dir_path = if scope.is_empty() {
                        first.to_owned()
                    } else {
                        format!("{scope}{first}")
                    };
                    dirs.insert(dir_path, ());
                }
                None => files.push(FileEntry {
                    path: artifact.name.clone(),
                    is_dir: false,
                    version: Some(artifact.version),
                }),
            }
        }
        let mut entries: Vec<FileEntry> = dirs
            .into_keys()
            .map(|path| FileEntry {
                path,
                is_dir: true,
                version: None,
            })
            .chain(files)
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, FsError> {
        let mut paths: Vec<String> = self
            .live_latest_set()
            .await
            .into_iter()
            .filter(|a| crate::policy::glob_match(pattern, &a.name))
            .map(|a| a.name)
            .collect();
        paths.sort();
        Ok(paths)
    }

    async fn grep(&self, pattern: &str, paths: Vec<String>) -> Result<Vec<GrepMatch>, FsError> {
        let candidates: Vec<Artifact> = if paths.is_empty() {
            self.live_latest_set().await
        } else {
            let mut out = Vec::new();
            for path in paths {
                let name = normalize_path(&path)?;
                if let Some(a) = self.latest_live(&name).await {
                    out.push(a);
                }
            }
            out
        };
        let mut matches = Vec::new();
        for artifact in candidates {
            for (index, line) in text_of(&artifact).split('\n').enumerate() {
                if line.contains(pattern) {
                    matches.push(GrepMatch {
                        path: artifact.name.clone(),
                        line_number: index + 1,
                        line_text: line.to_owned(),
                    });
                }
            }
        }
        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ailu_artifact_store::InMemoryArtifactStore;
    use ailu_graph_core::NodeId;

    use super::*;

    fn backend() -> ArtifactFsBackend {
        ArtifactFsBackend::new(Arc::new(InMemoryArtifactStore::new()), RunId::from("run-1"))
    }

    fn ctx() -> FsWriteCtx {
        FsWriteCtx {
            node_id: NodeId::from("agent"),
            principal: Some("agent".to_owned()),
        }
    }

    #[tokio::test]
    async fn write_then_read_round_trips_and_versions() {
        let fs = backend();
        let r1 = fs
            .write(
                "notes.md",
                "hello".to_owned(),
                ArtifactMediaType::TextMarkdown,
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(r1.version, 1);
        let r2 = fs
            .write(
                "notes.md",
                "hello world".to_owned(),
                ArtifactMediaType::TextMarkdown,
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(r2.version, 2);

        let latest = fs.read("notes.md", None).await.unwrap();
        assert_eq!(latest.content, "hello world");
        assert_eq!(latest.version, 2);
        let v1 = fs.read("notes.md", Some(1)).await.unwrap();
        assert_eq!(v1.content, "hello");
    }

    #[tokio::test]
    async fn read_missing_is_not_found() {
        let fs = backend();
        assert!(matches!(
            fs.read("nope.txt", None).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn edit_applies_line_patches_as_a_new_version() {
        let fs = backend();
        fs.write(
            "f.txt",
            "a\nb\nc".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();
        let r = fs
            .edit(
                "f.txt",
                vec![EditOp::Replace {
                    start_line: 2,
                    end_line: 2,
                    text: "B".to_owned(),
                }],
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(r.version, 2);
        assert_eq!(fs.read("f.txt", None).await.unwrap().content, "a\nB\nc");
    }

    #[tokio::test]
    async fn delete_tombstones_so_read_is_not_found_but_history_survives() {
        let fs = backend();
        fs.write(
            "x.txt",
            "data".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();
        fs.delete("x.txt", &ctx()).await.unwrap();
        assert!(matches!(
            fs.read("x.txt", None).await,
            Err(FsError::NotFound { .. })
        ));
        // The pre-delete version is still readable by version (audit trail intact).
        assert_eq!(fs.read("x.txt", Some(1)).await.unwrap().content, "data");
        // Deleting a non-existent file is not-found.
        assert!(matches!(
            fs.delete("ghost", &ctx()).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn rename_copies_then_tombstones_source() {
        let fs = backend();
        fs.write(
            "old.md",
            "content".to_owned(),
            ArtifactMediaType::TextMarkdown,
            &ctx(),
        )
        .await
        .unwrap();
        fs.rename("old.md", "new.md", &ctx()).await.unwrap();
        assert_eq!(fs.read("new.md", None).await.unwrap().content, "content");
        assert!(matches!(
            fs.read("old.md", None).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn ls_synthesizes_directories_and_lists_files() {
        let fs = backend();
        fs.write(
            "a/b/c.txt",
            "x".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();
        fs.write(
            "a/top.txt",
            "y".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();
        fs.write(
            "root.txt",
            "z".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();

        let root = fs.ls("").await.unwrap();
        // root: a synthetic dir "a" + the file "root.txt".
        assert!(root.iter().any(|e| e.path == "a" && e.is_dir));
        assert!(root.iter().any(|e| e.path == "root.txt" && !e.is_dir));

        let under_a = fs.ls("a").await.unwrap();
        assert!(under_a.iter().any(|e| e.path == "a/b" && e.is_dir));
        assert!(under_a.iter().any(|e| e.path == "a/top.txt" && !e.is_dir));
    }

    #[tokio::test]
    async fn glob_and_grep_over_live_files() {
        let fs = backend();
        fs.write(
            "src/a.rs",
            "fn main() {}\nlet x = 1;".to_owned(),
            ArtifactMediaType::TextPlain,
            &ctx(),
        )
        .await
        .unwrap();
        fs.write(
            "src/b.md",
            "# title".to_owned(),
            ArtifactMediaType::TextMarkdown,
            &ctx(),
        )
        .await
        .unwrap();
        fs.delete("src/b.md", &ctx()).await.unwrap();

        let rs = fs.glob("src/**/*.rs").await.unwrap();
        assert_eq!(rs, vec!["src/a.rs".to_owned()]);
        // The deleted markdown is excluded from glob.
        assert!(fs.glob("**/*.md").await.unwrap().is_empty());

        let hits = fs.grep("let x", vec![]).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/a.rs");
        assert_eq!(hits[0].line_number, 2);
    }

    #[tokio::test]
    async fn traversal_is_rejected() {
        let fs = backend();
        assert!(matches!(
            fs.write(
                "../escape",
                "x".to_owned(),
                ArtifactMediaType::TextPlain,
                &ctx()
            )
            .await,
            Err(FsError::InvalidPath { .. })
        ));
    }
}
