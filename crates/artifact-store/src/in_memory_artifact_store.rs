//! In-memory [`ArtifactStore`] — the Rust port of
//! `@ailu/artifact-store`'s `in-memory-artifact-store.ts`.
//!
//! Faithful to the TS versioning rule: an artifact's id is derived
//! deterministically from `runId:name`, the first write is version `1`, and
//! each later write of the same pair appends an incremented version.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ailu_graph_core::RunId;
use async_trait::async_trait;

use crate::interfaces::ArtifactStore;
use crate::types::{Artifact, ArtifactId, ArtifactVersion, ArtifactWriteInput};

/// A process-local, thread-safe [`ArtifactStore`].
///
/// Versions are kept in insertion order per artifact id behind a single
/// [`Mutex`], giving the whole type `Send + Sync` via interior mutability.
#[derive(Debug, Default)]
pub struct InMemoryArtifactStore {
    artifacts_by_id: Mutex<HashMap<ArtifactId, Vec<Artifact>>>,
}

impl InMemoryArtifactStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Derive the stable artifact id for a run/name pair (the TS `runId:name`).
fn create_artifact_id(run_id: &RunId, name: &str) -> ArtifactId {
    ArtifactId(format!("{run_id}:{name}"))
}

/// RFC 3339 (UTC) timestamp for the `createdAt` stamp. Falls back to the epoch
/// if the system clock is set before 1970.
fn now_rfc3339() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_epoch_rfc3339(duration.as_secs(), duration.subsec_millis())
}

/// Format seconds-since-epoch (plus milliseconds) as an RFC 3339 UTC string,
/// e.g. `2026-06-09T12:34:56.789Z`, without pulling in a date dependency.
fn format_epoch_rfc3339(secs: u64, millis: u32) -> String {
    // Days since the Unix epoch and the time-of-day remainder.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);

    // Civil-from-days algorithm (Howard Hinnant) for the calendar date.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[async_trait]
impl ArtifactStore for InMemoryArtifactStore {
    async fn write(&self, input: ArtifactWriteInput) -> Artifact {
        let id = create_artifact_id(&input.run_id, &input.name);
        let mut artifacts = self
            .artifacts_by_id
            .lock()
            .expect("artifact store poisoned");
        let versions = artifacts.entry(id.clone()).or_default();
        let next_version = versions.last().map(|a| a.version).unwrap_or(0) + 1;

        let artifact = Artifact {
            id,
            run_id: input.run_id,
            node_id: input.node_id,
            name: input.name,
            media_type: input.media_type,
            version: next_version,
            content: input.content,
            created_at: now_rfc3339(),
            metadata: input.metadata,
        };

        versions.push(artifact.clone());
        artifact
    }

    async fn read(&self, id: &ArtifactId) -> Option<Artifact> {
        let artifacts = self
            .artifacts_by_id
            .lock()
            .expect("artifact store poisoned");
        artifacts
            .get(id)
            .and_then(|versions| versions.last().cloned())
    }

    async fn read_version(&self, id: &ArtifactId, version: ArtifactVersion) -> Option<Artifact> {
        let artifacts = self
            .artifacts_by_id
            .lock()
            .expect("artifact store poisoned");
        artifacts
            .get(id)
            .and_then(|versions| versions.iter().find(|a| a.version == version).cloned())
    }

    async fn list_by_run(&self, run_id: &RunId) -> Vec<Artifact> {
        let artifacts = self
            .artifacts_by_id
            .lock()
            .expect("artifact store poisoned");
        artifacts
            .values()
            .flatten()
            .filter(|artifact| &artifact.run_id == run_id)
            .cloned()
            .collect()
    }

    async fn list_versions(&self, id: &ArtifactId) -> Vec<Artifact> {
        let artifacts = self
            .artifacts_by_id
            .lock()
            .expect("artifact store poisoned");
        artifacts.get(id).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ArtifactMediaType;
    use ailu_graph_core::NodeId;

    fn run_id() -> RunId {
        RunId::from("run-1")
    }

    fn node_id() -> NodeId {
        NodeId::from("node-1")
    }

    fn input(
        name: &str,
        media_type: ArtifactMediaType,
        content: serde_json::Value,
    ) -> ArtifactWriteInput {
        ArtifactWriteInput {
            run_id: run_id(),
            node_id: node_id(),
            name: name.to_owned(),
            media_type,
            content,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn write_creates_an_artifact_with_version_1() {
        let store = InMemoryArtifactStore::new();

        let artifact = store
            .write(input(
                "analysis",
                ArtifactMediaType::ApplicationJson,
                serde_json::json!({ "score": 1 }),
            ))
            .await;

        assert_eq!(artifact.version, 1);
        assert_eq!(artifact.name, "analysis");
    }

    #[tokio::test]
    async fn write_with_same_run_id_and_name_increments_version_to_2() {
        let store = InMemoryArtifactStore::new();

        let first = store
            .write(input(
                "analysis",
                ArtifactMediaType::ApplicationJson,
                serde_json::json!({ "score": 1 }),
            ))
            .await;
        let second = store
            .write(input(
                "analysis",
                ArtifactMediaType::ApplicationJson,
                serde_json::json!({ "score": 2 }),
            ))
            .await;

        assert_eq!(first.id, second.id);
        assert_eq!(second.version, 2);
    }

    #[tokio::test]
    async fn read_returns_the_latest_artifact_version() {
        let store = InMemoryArtifactStore::new();
        let first = store
            .write(input(
                "report",
                ArtifactMediaType::TextMarkdown,
                serde_json::json!("# v1"),
            ))
            .await;
        store
            .write(input(
                "report",
                ArtifactMediaType::TextMarkdown,
                serde_json::json!("# v2"),
            ))
            .await;

        let latest = store.read(&first.id).await;

        assert_eq!(latest.as_ref().map(|a| a.version), Some(2));
        assert_eq!(latest.map(|a| a.content), Some(serde_json::json!("# v2")));
    }

    #[tokio::test]
    async fn read_version_returns_the_exact_requested_version() {
        let store = InMemoryArtifactStore::new();
        let first = store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v1"),
            ))
            .await;
        store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v2"),
            ))
            .await;

        let v1 = store.read_version(&first.id, 1).await;

        assert_eq!(v1.as_ref().map(|a| a.version), Some(1));
        assert_eq!(v1.map(|a| a.content), Some(serde_json::json!("v1")));
    }

    #[tokio::test]
    async fn read_and_read_version_miss_return_none() {
        let store = InMemoryArtifactStore::new();
        let missing = ArtifactId::from("run-1:nope");

        assert!(store.read(&missing).await.is_none());

        store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v1"),
            ))
            .await;
        let id = ArtifactId::from("run-1:report");
        assert!(store.read_version(&id, 99).await.is_none());
    }

    #[tokio::test]
    async fn list_by_run_returns_all_artifacts_for_a_run() {
        let store = InMemoryArtifactStore::new();
        store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v1"),
            ))
            .await;
        store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v2"),
            ))
            .await;
        store
            .write(input(
                "raw",
                ArtifactMediaType::ApplicationOctetStream,
                serde_json::json!([1, 2, 3]),
            ))
            .await;

        let artifacts = store.list_by_run(&run_id()).await;

        assert_eq!(artifacts.len(), 3);
    }

    #[tokio::test]
    async fn list_versions_returns_all_versions_for_an_artifact_id() {
        let store = InMemoryArtifactStore::new();
        let first = store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v1"),
            ))
            .await;
        store
            .write(input(
                "report",
                ArtifactMediaType::TextPlain,
                serde_json::json!("v2"),
            ))
            .await;

        let versions = store.list_versions(&first.id).await;

        assert_eq!(
            versions.iter().map(|a| a.version).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn format_epoch_rfc3339_matches_known_timestamps() {
        // 2026-06-09T00:00:00.000Z == 1_780_963_200 seconds since the epoch.
        assert_eq!(
            format_epoch_rfc3339(1_780_963_200, 0),
            "2026-06-09T00:00:00.000Z"
        );
        // A timestamp with hours/minutes/seconds and sub-second millis.
        // 2026-06-06T12:34:56.789Z == 1_780_749_296 seconds since the epoch.
        assert_eq!(
            format_epoch_rfc3339(1_780_749_296, 789),
            "2026-06-06T12:34:56.789Z"
        );
        // The Unix epoch itself.
        assert_eq!(format_epoch_rfc3339(0, 0), "1970-01-01T00:00:00.000Z");
    }
}
