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
use crate::redactor::{redactable_texts, write_back_redacted, PiiRedactor};
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

/// ADR 0058: one call to the PII seam, as the recorded run made it. The seam is an external,
/// non-deterministic service whose output shapes every request journaled after it, so a replay
/// re-feeds that output instead of calling the seam again. Only the seam's OUTPUT is kept:
/// `texts[i]` is what the seam returned for the request's i-th redactable text, or `None` when it
/// returned that text unchanged, so the journal never holds a text the seam rewrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedRedaction {
    /// The run the request belongs to (ADR 0043), so concurrent spawns and subgraph children
    /// keep their own sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The agent node whose stack called the seam.
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<Option<String>>,
    /// The reason when the seam blocked the call (`LlmError::PiiBlocked`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

/// Wraps the live PII seam in record mode: redacts through `inner`, then journals the seam's
/// output (never its input) into a run-scoped log shared by every agent of the run.
pub struct RecordingPiiRedactor {
    inner: Arc<dyn PiiRedactor>,
    node_id: String,
    log: Arc<Mutex<Vec<RecordedRedaction>>>,
}

impl RecordingPiiRedactor {
    pub fn new(
        inner: Arc<dyn PiiRedactor>,
        node_id: &str,
        log: Arc<Mutex<Vec<RecordedRedaction>>>,
    ) -> Self {
        Self {
            inner,
            node_id: node_id.to_owned(),
            log,
        }
    }
}

#[async_trait]
impl PiiRedactor for RecordingPiiRedactor {
    async fn redact_request(&self, request: LlmRequest) -> Result<LlmRequest, LlmError> {
        let run_id = request.run_id.clone();
        let before = redactable_texts(&request);
        let outcome = self.inner.redact_request(request).await;
        let (texts, blocked) = match &outcome {
            Ok(redacted) => (
                redactable_texts(redacted)
                    .into_iter()
                    .zip(&before)
                    .map(|(after, before)| (&after != before).then_some(after))
                    .collect(),
                None,
            ),
            Err(LlmError::PiiBlocked(reason)) => (Vec::new(), Some(reason.clone())),
            Err(_) => return outcome,
        };
        self.log
            .lock()
            .expect("recording seam mutex poisoned")
            .push(RecordedRedaction {
                run_id,
                node_id: self.node_id.clone(),
                texts,
                blocked,
            });
        outcome
    }

    async fn hydrate_response(&self, response: LlmResponse) -> LlmResponse {
        self.inner.hydrate_response(response).await
    }
}

/// The recorded seam outputs of a run, each consumed once, in order per `(run_id, node_id)`.
pub struct RedactionReplayLog {
    calls: Mutex<Vec<(RecordedRedaction, bool)>>,
}

impl RedactionReplayLog {
    pub fn new(calls: Vec<RecordedRedaction>) -> Self {
        Self {
            calls: Mutex::new(calls.into_iter().map(|c| (c, false)).collect()),
        }
    }

    fn take(&self, run_id: Option<&str>, node_id: &str) -> Option<RecordedRedaction> {
        let mut calls = self.calls.lock().expect("seam replay mutex poisoned");
        let (call, consumed) = calls.iter_mut().find(|(call, consumed)| {
            !*consumed && call.run_id.as_deref() == run_id && call.node_id == node_id
        })?;
        *consumed = true;
        Some(call.clone())
    }
}

/// Stands in for the PII seam on a replay of a run that recorded it: re-feeds the recorded output
/// and never calls a service. A call with no recorded output fails with
/// [`LlmError::ReplayJournalMiss`]: the request is never passed on unredacted.
pub struct ReplayPiiRedactor {
    node_id: String,
    log: Arc<RedactionReplayLog>,
}

impl ReplayPiiRedactor {
    pub fn new(node_id: &str, log: Arc<RedactionReplayLog>) -> Self {
        Self {
            node_id: node_id.to_owned(),
            log,
        }
    }
}

#[async_trait]
impl PiiRedactor for ReplayPiiRedactor {
    async fn redact_request(&self, mut request: LlmRequest) -> Result<LlmRequest, LlmError> {
        // Messages name the call only: never a text, which is the unredacted input here.
        let Some(recorded) = self.log.take(request.run_id.as_deref(), &self.node_id) else {
            return Err(LlmError::ReplayJournalMiss(format!(
                "no recorded PII seam output for run={:?} node={}",
                request.run_id, self.node_id
            )));
        };
        if let Some(reason) = recorded.blocked {
            return Err(LlmError::PiiBlocked(reason));
        }
        let texts = redactable_texts(&request);
        if texts.len() != recorded.texts.len() {
            return Err(LlmError::ReplayJournalMiss(format!(
                "PII seam output recorded for {} texts, request has {} (run={:?} node={})",
                recorded.texts.len(),
                texts.len(),
                request.run_id,
                self.node_id
            )));
        }
        let redacted = texts
            .into_iter()
            .zip(recorded.texts)
            .map(|(text, recorded)| recorded.unwrap_or(text))
            .collect();
        write_back_redacted(&mut request, redacted);
        Ok(request)
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

    /// A seam that masks one name, as a real one rewrites the entities it detects.
    struct MaskingSeam;

    #[async_trait]
    impl PiiRedactor for MaskingSeam {
        async fn redact_request(&self, mut request: LlmRequest) -> Result<LlmRequest, LlmError> {
            for message in request.messages.iter_mut() {
                message.content = message.content.replace("Jeanne Martin", "[SEAM_PERSON_1]");
            }
            Ok(request)
        }
    }

    fn two_texts(first: &str, second: &str) -> LlmRequest {
        let mut request = req_for_run(first, Some("run-1"));
        request.messages.push(LlmMessage {
            role: "user".to_owned(),
            content: second.to_owned(),
            ..Default::default()
        });
        request
    }

    #[tokio::test]
    async fn a_replay_without_the_seam_re_feeds_its_recorded_output() {
        // ADR 0058: record through a seam that rewrites, replay with no seam at all.
        let log = Arc::new(Mutex::new(Vec::new()));
        let recording = RecordingPiiRedactor::new(Arc::new(MaskingSeam), "agent", Arc::clone(&log));
        let raw = two_texts("Relance Jeanne Martin", "Facture inv-42");
        let sent = recording.redact_request(raw.clone()).await.unwrap();
        assert_eq!(sent.messages[0].content, "Relance [SEAM_PERSON_1]");

        // Only the seam's output is journaled: the rewritten text, `None` for the unchanged one.
        let recorded = log.lock().unwrap().clone();
        let wire = serde_json::to_string(&recorded).unwrap();
        assert!(
            !wire.contains("Jeanne Martin"),
            "pre-seam text journaled: {wire}"
        );
        assert_eq!(
            recorded[0].texts,
            vec![Some("Relance [SEAM_PERSON_1]".to_owned()), None]
        );

        let replay = ReplayPiiRedactor::new(
            "agent",
            Arc::new(RedactionReplayLog::new(
                serde_json::from_str(&wire).unwrap(),
            )),
        );
        assert_eq!(replay.redact_request(raw.clone()).await.unwrap(), sent);
        // Each recorded output serves once: a further call has none, and fails typed.
        match replay.redact_request(raw).await {
            Err(LlmError::ReplayJournalMiss(message)) => {
                assert!(!message.contains("Jeanne"), "{message}");
            }
            other => panic!("expected ReplayJournalMiss, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_replay_seam_never_passes_an_unrecorded_request_on() {
        let replay = ReplayPiiRedactor::new("agent", Arc::new(RedactionReplayLog::new(Vec::new())));
        match replay.redact_request(two_texts("Jeanne Martin", "x")).await {
            Err(LlmError::ReplayJournalMiss(_)) => {}
            other => panic!("expected ReplayJournalMiss, got {other:?}"),
        }
        // Another node's output, or one recorded for a different number of texts, is no match.
        let other_node = ReplayPiiRedactor::new(
            "agent",
            Arc::new(RedactionReplayLog::new(vec![
                RecordedRedaction {
                    run_id: Some("run-1".to_owned()),
                    node_id: "other".to_owned(),
                    texts: vec![None, None],
                    blocked: None,
                },
                RecordedRedaction {
                    run_id: Some("run-1".to_owned()),
                    node_id: "agent".to_owned(),
                    texts: vec![None],
                    blocked: None,
                },
            ])),
        );
        match other_node
            .redact_request(two_texts("Jeanne Martin", "x"))
            .await
        {
            Err(LlmError::ReplayJournalMiss(_)) => {}
            other => panic!("expected ReplayJournalMiss, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_recorded_block_replays_as_a_block() {
        let replay = ReplayPiiRedactor::new(
            "agent",
            Arc::new(RedactionReplayLog::new(vec![RecordedRedaction {
                run_id: Some("run-1".to_owned()),
                node_id: "agent".to_owned(),
                texts: Vec::new(),
                blocked: Some("personal data".to_owned()),
            }])),
        );
        assert_eq!(
            replay.redact_request(two_texts("a", "b")).await,
            Err(LlmError::PiiBlocked("personal data".to_owned()))
        );
    }
}
