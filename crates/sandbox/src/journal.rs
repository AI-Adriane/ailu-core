//! The guard's own durable record of sandbox transitions.
//!
//! One transition, one event: `sandbox_created`, `sandbox_exec_started`,
//! `sandbox_exec_completed`, `sandbox_exec_failed`, `sandbox_collected`,
//! `sandbox_destroyed` and `sandbox_denied`. Events carry identifiers and digests only, never
//! content. They live here, not in the run event stream, until that stream can grow a variant
//! without breaking its consumers.
//!
//! The journal deduplicates: the same idempotency key with the same event kind (and, for
//! refusals, the same code) is recorded once, so replaying a node does not write its history
//! again.

use crate::ids::{IsolationDomain, SandboxId, TenantId};
use ailu_graph_core::{NodeId, RunId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Mutex;

/// The seven transitions the guard records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEventKind {
    SandboxCreated,
    SandboxExecStarted,
    SandboxExecCompleted,
    SandboxExecFailed,
    SandboxCollected,
    SandboxDestroyed,
    SandboxDenied,
}

impl SandboxEventKind {
    /// Stable wire name, the same as the serialized form.
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxEventKind::SandboxCreated => "sandbox_created",
            SandboxEventKind::SandboxExecStarted => "sandbox_exec_started",
            SandboxEventKind::SandboxExecCompleted => "sandbox_exec_completed",
            SandboxEventKind::SandboxExecFailed => "sandbox_exec_failed",
            SandboxEventKind::SandboxCollected => "sandbox_collected",
            SandboxEventKind::SandboxDestroyed => "sandbox_destroyed",
            SandboxEventKind::SandboxDenied => "sandbox_denied",
        }
    }
}

/// One recorded transition. Identifiers and digests only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxEvent {
    pub kind: SandboxEventKind,
    pub tenant_id: TenantId,
    pub run_id: RunId,
    pub node_id: NodeId,
    pub attempt: u32,
    /// Absent when no sandbox exists (a refusal before `create`).
    pub sandbox_id: Option<SandboxId>,
    /// Idempotency key of the operation, lowercase hex.
    pub key: String,
    /// Fingerprint of the request the transition is about.
    pub request_digest: Option<String>,
    /// Digest of what the operation produced (the outcome, or the outputs).
    pub result_digest: Option<String>,
    /// The isolation domain the backend declared, on `sandbox_created`.
    pub isolation_domain: Option<IsolationDomain>,
    /// Payload hash of the approval that authorized the call, as the verifier gave it (on
    /// `sandbox_created` and `sandbox_exec_started`).
    #[serde(default)]
    pub approval_payload_hash: Option<String>,
    /// Full error code, on `sandbox_exec_failed` and `sandbox_denied`.
    pub code: Option<String>,
    /// Time of the transition, from the injected clock.
    pub at_ms: u64,
}

impl SandboxEvent {
    /// Identity used for deduplication: the key, the kind and the code.
    pub fn dedupe_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.key,
            self.kind.as_str(),
            self.code.as_deref().unwrap_or("")
        )
    }
}

/// Whether an append wrote a new event or found the same one already recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appended {
    New,
    Duplicate,
}

/// The journal could not record the event. The guard then fails closed with a retriable
/// `Unavailable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("sandbox journal unavailable")]
pub struct JournalError;

/// Durable, append-only record of sandbox transitions. An implementation must deduplicate by
/// [`SandboxEvent::dedupe_key`] and keep events per tenant isolated from one another.
#[async_trait]
pub trait SandboxJournal: Send + Sync {
    async fn append(&self, event: SandboxEvent) -> Result<Appended, JournalError>;
}

/// In-memory journal for tests and single-process use. Not durable.
#[derive(Default)]
pub struct InMemoryJournal {
    state: Mutex<(Vec<SandboxEvent>, HashSet<String>)>,
}

impl InMemoryJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Events in the order they were first recorded.
    pub fn events(&self) -> Vec<SandboxEvent> {
        self.lock(|state| state.0.clone())
    }

    pub fn len(&self) -> usize {
        self.lock(|state| state.0.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock<T>(&self, f: impl FnOnce(&mut (Vec<SandboxEvent>, HashSet<String>)) -> T) -> T {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }
}

#[async_trait]
impl SandboxJournal for InMemoryJournal {
    async fn append(&self, event: SandboxEvent) -> Result<Appended, JournalError> {
        Ok(self.lock(|state| {
            if state.1.insert(event.dedupe_key()) {
                state.0.push(event);
                Appended::New
            } else {
                Appended::Duplicate
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: SandboxEventKind, key: &str, code: Option<&str>) -> SandboxEvent {
        SandboxEvent {
            kind,
            tenant_id: TenantId::new("t"),
            run_id: RunId::from("r"),
            node_id: NodeId::from("n"),
            attempt: 1,
            sandbox_id: Some(SandboxId::new("sb")),
            key: key.to_owned(),
            request_digest: None,
            result_digest: None,
            isolation_domain: None,
            approval_payload_hash: None,
            code: code.map(str::to_owned),
            at_ms: 5,
        }
    }

    #[tokio::test]
    async fn duplicates_are_recorded_once() {
        let journal = InMemoryJournal::new();
        let e = event(SandboxEventKind::SandboxCreated, "k1", None);
        assert_eq!(journal.append(e.clone()).await, Ok(Appended::New));
        assert_eq!(journal.append(e).await, Ok(Appended::Duplicate));
        assert_eq!(journal.len(), 1);
    }

    #[tokio::test]
    async fn key_kind_and_code_each_make_a_distinct_event() {
        let journal = InMemoryJournal::new();
        let all = [
            event(SandboxEventKind::SandboxDenied, "k1", Some("a")),
            event(SandboxEventKind::SandboxDenied, "k1", Some("b")),
            event(SandboxEventKind::SandboxDenied, "k2", Some("a")),
            event(SandboxEventKind::SandboxCreated, "k1", None),
        ];
        for e in all {
            assert_eq!(journal.append(e).await, Ok(Appended::New));
        }
        assert_eq!(journal.len(), 4);
        assert!(!journal.is_empty());
    }

    #[tokio::test]
    async fn events_keep_their_first_recording_order() {
        let journal = InMemoryJournal::new();
        for kind in [
            SandboxEventKind::SandboxCreated,
            SandboxEventKind::SandboxExecStarted,
            SandboxEventKind::SandboxExecCompleted,
        ] {
            journal.append(event(kind, "k", None)).await.unwrap();
        }
        journal
            .append(event(SandboxEventKind::SandboxCreated, "k", None))
            .await
            .unwrap();
        let kinds: Vec<_> = journal.events().iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SandboxEventKind::SandboxCreated,
                SandboxEventKind::SandboxExecStarted,
                SandboxEventKind::SandboxExecCompleted
            ]
        );
    }

    #[test]
    fn wire_names_match_the_serialized_form() {
        for kind in [
            SandboxEventKind::SandboxCreated,
            SandboxEventKind::SandboxExecStarted,
            SandboxEventKind::SandboxExecCompleted,
            SandboxEventKind::SandboxExecFailed,
            SandboxEventKind::SandboxCollected,
            SandboxEventKind::SandboxDestroyed,
            SandboxEventKind::SandboxDenied,
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }
}
