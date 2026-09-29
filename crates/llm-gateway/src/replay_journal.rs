//! Replay-as-evidence (ADR 0038) — the LLM I/O journal + the record/replay gateways.
//!
//! Deterministic re-execution needs the original run's LLM outputs re-fed instead of
//! re-sampled (an LLM is stochastic). Two [`LlmGateway`] decorators provide that, mirroring
//! the [`crate::compressor`] / [`crate::redactor`] seams:
//!
//! - [`RecordingGateway`] wraps the live gateway and journals every `(request, response)`
//!   pair AFTER the inner call. Compose it OUTERMOST — after redaction (ADR 0032) — so the
//!   journal holds the already-scrubbed request and never becomes a secret sink.
//! - [`ReplayGateway`] is driven by a recorded [`LlmJournal`] alone: every call returns the
//!   recorded response for the matching request WITHOUT touching any provider. A request with
//!   no recorded match is a journal gap ([`LlmError::ReplayJournalMiss`]) — a replay must
//!   never silently fall through to a live call.
//!
//! Matching is by request equality, consuming each recorded call once in occurrence order, so
//! repeated identical requests replay their distinct responses in the order they were made.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::gateway::{LlmGateway, TokenSink};
use crate::types::{LlmRequest, LlmResponse};

/// One recorded LLM call: the request sent and the response the provider returned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedCall {
    pub request: LlmRequest,
    pub response: LlmResponse,
}

/// The full LLM I/O journal for a run — the ordered list of every `(request, response)`
/// pair (the locked "full LLM I/O per run" decision, ADR 0038). Serializable so the control
/// plane can persist it alongside the run and re-feed it on a replay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmJournal {
    pub calls: Vec<RecordedCall>,
}

/// Wraps any [`LlmGateway`], recording each `complete`/`stream` `(request, response)` into a
/// shared journal after the inner gateway returns. Otherwise transparent.
pub struct RecordingGateway {
    inner: Arc<dyn LlmGateway>,
    journal: Arc<Mutex<Vec<RecordedCall>>>,
}

impl RecordingGateway {
    pub fn new(inner: Arc<dyn LlmGateway>) -> Self {
        Self {
            inner,
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Wrap `inner`, recording into a CALLER-OWNED shared journal — so several agent gateways
    /// in one run all append to a single run-scoped journal (ADR 0038, layer 2b). The caller
    /// holds the same `Arc` and reads it after the run to persist the full LLM I/O.
    pub fn with_journal(
        inner: Arc<dyn LlmGateway>,
        journal: Arc<Mutex<Vec<RecordedCall>>>,
    ) -> Self {
        Self { inner, journal }
    }

    /// A shared handle to the recorded calls — clone it before the run, read it after to
    /// build the [`LlmJournal`] to persist.
    pub fn journal_handle(&self) -> Arc<Mutex<Vec<RecordedCall>>> {
        Arc::clone(&self.journal)
    }

    /// Snapshot the journal recorded so far.
    pub fn journal(&self) -> LlmJournal {
        LlmJournal {
            calls: self
                .journal
                .lock()
                .expect("recording journal mutex poisoned")
                .clone(),
        }
    }

    fn record(&self, request: LlmRequest, response: &LlmResponse) {
        self.journal
            .lock()
            .expect("recording journal mutex poisoned")
            .push(RecordedCall {
                request,
                response: response.clone(),
            });
    }
}

#[async_trait]
impl LlmGateway for RecordingGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let response = self.inner.complete(request.clone()).await?;
        self.record(request, &response);
        Ok(response)
    }

    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let response = self.inner.stream(request.clone(), on_delta).await?;
        self.record(request, &response);
        Ok(response)
    }
}

/// Replays a recorded [`LlmJournal`]: returns the recorded response for the matching request,
/// never calling a provider. There is no inner gateway — a missing match is an error, so a
/// replay can never re-sample.
pub struct ReplayGateway {
    /// `(call, consumed)` — each recorded entry is consumed once, matched by request, so
    /// repeated identical requests replay their distinct responses in occurrence order.
    calls: Mutex<Vec<(RecordedCall, bool)>>,
}

impl ReplayGateway {
    pub fn new(journal: LlmJournal) -> Self {
        Self {
            calls: Mutex::new(journal.calls.into_iter().map(|c| (c, false)).collect()),
        }
    }

    fn take_matching(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut calls = self.calls.lock().expect("replay journal mutex poisoned");
        for (call, consumed) in calls.iter_mut() {
            if !*consumed && &call.request == request {
                *consumed = true;
                return Ok(call.response.clone());
            }
        }
        // Keep the message free of full prompt content (which could be large / sensitive):
        // identify the request by provider + model + message count only.
        Err(LlmError::ReplayJournalMiss(format!(
            "provider={:?} model={} messages={}",
            request.provider,
            request.model,
            request.messages.len()
        )))
    }
}

#[async_trait]
impl LlmGateway for ReplayGateway {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        self.take_matching(&request)
    }

    async fn stream(
        &self,
        request: LlmRequest,
        on_delta: &TokenSink<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let response = self.take_matching(&request)?;
        // Honour the streaming contract chunk-once: a replay re-emits the recorded content
        // as a single delta, returning the same assembled response.
        if !response.content.is_empty() {
            on_delta(&response.content);
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::gateway::DefaultLlmGateway;
    use crate::mock::MockAdapter;
    use crate::types::{LlmMessage, LlmProvider, LlmResponse, LlmUsage};

    fn req(content: &str) -> LlmRequest {
        req_for_run(content, None)
    }

    /// Same as [`req`], but tagged for a specific run (ADR 0043) — lets tests exercise the
    /// run-scoped matching a recursive (subgraph) replay depends on.
    fn req_for_run(content: &str, run_id: Option<&str>) -> LlmRequest {
        LlmRequest {
            web_search: None,
            provider: LlmProvider::Openai,
            model: "m".to_owned(),
            messages: vec![LlmMessage {
                role: "user".to_owned(),
                content: content.to_owned(),
                tool_calls: None,
                ..Default::default()
            }],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: run_id.map(str::to_owned),
        }
    }

    fn resp(content: &str) -> LlmResponse {
        LlmResponse {
            web_search: None,
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("stop".to_owned()),
            usage: LlmUsage::default(),
            model: "m".to_owned(),
            provider: LlmProvider::Openai,
            content_blocks: None,
        }
    }

    fn recording_over_mock(responses: Vec<LlmResponse>) -> RecordingGateway {
        let mut gw = DefaultLlmGateway::new();
        gw.register_adapter(Box::new(MockAdapter::new(LlmProvider::Openai, responses)));
        RecordingGateway::new(Arc::new(gw))
    }

    #[tokio::test]
    async fn recording_gateway_journals_each_call_in_order() {
        let rec = recording_over_mock(vec![resp("r1"), resp("r2")]);
        let a = rec.complete(req("a")).await.unwrap();
        let b = rec.complete(req("b")).await.unwrap();
        assert_eq!(a.content, "r1");
        assert_eq!(b.content, "r2");

        let journal = rec.journal();
        assert_eq!(journal.calls.len(), 2);
        assert_eq!(journal.calls[0].request, req("a"));
        assert_eq!(journal.calls[0].response.content, "r1");
        assert_eq!(journal.calls[1].request, req("b"));
        assert_eq!(journal.calls[1].response.content, "r2");
    }

    #[tokio::test]
    async fn replay_gateway_returns_recorded_responses_without_a_provider() {
        // Record a journal, then replay it WITHOUT any inner gateway — a provider call would
        // be impossible here, proving the replay is served purely from the journal.
        let rec = recording_over_mock(vec![resp("r1"), resp("r2")]);
        rec.complete(req("a")).await.unwrap();
        rec.complete(req("b")).await.unwrap();
        let journal = rec.journal();

        let replay = ReplayGateway::new(journal);
        assert_eq!(replay.complete(req("a")).await.unwrap().content, "r1");
        assert_eq!(replay.complete(req("b")).await.unwrap().content, "r2");
        // A request with no recorded match is a journal gap, never a silent re-sample.
        match replay.complete(req("unseen")).await {
            Err(LlmError::ReplayJournalMiss(_)) => {}
            other => panic!("expected ReplayJournalMiss, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn recording_gateway_journals_the_run_id_set_on_the_request() {
        let rec = recording_over_mock(vec![resp("r1")]);
        rec.complete(req_for_run("a", Some("run-1:node-1")))
            .await
            .unwrap();
        let journal = rec.journal();
        assert_eq!(
            journal.calls[0].request.run_id.as_deref(),
            Some("run-1:node-1")
        );
    }

    #[tokio::test]
    async fn replay_gateway_discriminates_identical_requests_by_run_id() {
        // ADR 0043 D2: a parent run and its subgraph child can make byte-identical LLM
        // calls (same content). Tagging each request with its own run_id (D1) lets the
        // EXISTING request == request equality match in take_matching discriminate them
        // — no separate journal-partitioning logic needed.
        let journal = LlmJournal {
            calls: vec![
                RecordedCall {
                    request: req_for_run("x", Some("run-1")),
                    response: resp("parent-answer"),
                },
                RecordedCall {
                    request: req_for_run("x", Some("run-1:node-1")),
                    response: resp("child-answer"),
                },
            ],
        };
        let replay = ReplayGateway::new(journal);

        assert_eq!(
            replay
                .complete(req_for_run("x", Some("run-1:node-1")))
                .await
                .unwrap()
                .content,
            "child-answer"
        );
        assert_eq!(
            replay
                .complete(req_for_run("x", Some("run-1")))
                .await
                .unwrap()
                .content,
            "parent-answer"
        );
        // An untagged request (run_id: None) matches neither run-tagged entry — proving
        // discrimination is real, not a coincidence of consumption order.
        match replay.complete(req("x")).await {
            Err(LlmError::ReplayJournalMiss(_)) => {}
            other => panic!("expected ReplayJournalMiss, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn replay_gateway_stream_reemits_recorded_content_as_one_delta() {
        let rec = recording_over_mock(vec![resp("hello world")]);
        rec.complete(req("a")).await.unwrap();
        let replay = ReplayGateway::new(rec.journal());

        let deltas = Mutex::new(Vec::<String>::new());
        let count = AtomicUsize::new(0);
        let sink = |d: &str| {
            count.fetch_add(1, Ordering::SeqCst);
            deltas.lock().unwrap().push(d.to_owned());
        };
        let out = replay.stream(req("a"), &sink).await.unwrap();
        assert_eq!(out.content, "hello world");
        assert_eq!(count.load(Ordering::SeqCst), 1); // chunk-once
        assert_eq!(
            deltas.lock().unwrap().as_slice(),
            &["hello world".to_owned()]
        );
    }
}
