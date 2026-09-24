//! Reflection — the Rust port of `@ailu/agents-core`'s `createReflectionNode`
//! (`reflection-node.ts`).
//!
//! The TS ships a graph node that runs **one** critique per invocation and either
//! loops back to a previous node (to re-draft) or accepts the current output. The
//! looping is driven by the runtime; the per-round contract is:
//!
//! 1. critique the current output with an LLM call (asking for a structured
//!    `{ ok, score, issues }` verdict);
//! 2. if the budget is not yet exhausted **and** the critique rejects the draft —
//!    structured: `!ok && score < scoreThreshold`; or, when the reply is not JSON, the
//!    legacy fallback `contains("problem") || contains("retry")` — revise (loop back,
//!    round + 1);
//! 3. otherwise accept — keep the output and bump `confidence` by `0.1` (capped 1.0).
//!
//! `__reflectionCount` carries the round across invocations; the default budget is
//! `maxReflections = 2` (i.e. up to two revisions after the first draft).
//!
//! This module collapses that runtime-driven loop into one in-process driver:
//! [`ReflectionAgent::run`] takes an initial draft plus a `revise` seam (the Rust
//! analogue of "loop back to the previous node to produce a new draft") and repeats
//! critique → revise until the critique accepts or the budget is spent. It returns
//! the final draft, the rounds taken, and the confidence the TS would have set.

use std::sync::Arc;

use ailu_llm_gateway::{LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest};
use serde::{Deserialize, Serialize};

/// Model used for the critique call. Matches the TS `model: "reflection-node"`.
pub const REFLECTION_MODEL: &str = "reflection-node";

/// Default round budget. Matches the TS `maxReflections ?? 2`.
pub const DEFAULT_MAX_REFLECTIONS: usize = 2;

/// Accept a structured critique whose `score` is at least this (0..1). Matches the TS
/// `scoreThreshold ?? 0.8`.
pub const DEFAULT_SCORE_THRESHOLD: f64 = 0.8;

/// Substrings (checked against the lowercased critique) that signal the draft is not yet
/// acceptable and must be revised — the *fallback* heuristic, used only when the critique is not
/// structured JSON. Mirrors the TS `critique.includes("problem") || critique.includes("retry")`.
pub const REVISE_MARKERS: [&str; 2] = ["problem", "retry"];

/// Instruction prepended to the critique prompt asking for a structured verdict. Mirrors the TS
/// `CRITIQUE_INSTRUCTION`.
const CRITIQUE_INSTRUCTION: &str = "Critique the output below. Respond ONLY with JSON of the form \
{\"ok\": boolean, \"score\": number between 0 and 1, \"issues\": string[]}. \
`ok` is true when the output is acceptable as-is; `score` is overall quality; \
`issues` lists concrete, actionable problems to fix.";

/// Structured critique the reflection model is asked to return. Mirrors the TS
/// `ReflectionCritique`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Critique {
    pub ok: bool,
    pub score: f64,
    pub issues: Vec<String>,
}

/// Lenient wire shape: every field optional so a partial `{score:…}` or `{ok:…}` still parses.
#[derive(Deserialize)]
struct CritiqueWire {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    issues: Vec<String>,
}

/// Tolerantly extract a {@link Critique} from a model reply — JSON possibly wrapped in prose or a
/// markdown fence. Returns `None` when no structured critique is present (caller falls back to the
/// substring heuristic). Mirrors the TS `parseReflectionCritique`.
pub fn parse_critique(raw: &str) -> Option<Critique> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    let wire: CritiqueWire = serde_json::from_str(&raw[start..=end]).ok()?;
    if wire.ok.is_none() && wire.score.is_none() {
        return None; // a JSON object that isn't a critique
    }
    let ok = wire.ok.unwrap_or(false);
    let score = wire
        .score
        .map(|s| s.clamp(0.0, 1.0))
        .unwrap_or(if ok { 1.0 } else { 0.0 });
    Some(Critique {
        ok,
        score,
        issues: wire.issues,
    })
}

/// Decide whether the draft needs another revision, plus the concrete issues to fix. A structured
/// critique revises unless `ok` or `score >= score_threshold`; otherwise the legacy substring
/// heuristic applies (no issues). Mirrors the TS `critiqueRequestsRevision`.
pub fn requests_revision(raw: &str, score_threshold: f64) -> (bool, Vec<String>) {
    if let Some(critique) = parse_critique(raw) {
        let accept = critique.ok || critique.score >= score_threshold;
        return (!accept, critique.issues);
    }
    let lower = raw.to_lowercase();
    let revise = REVISE_MARKERS.iter().any(|marker| lower.contains(marker));
    (revise, Vec::new())
}

/// Run ONE critique pass over `draft` and report whether it requests a revision, plus the
/// concrete issues. The single-shot core shared by [`ReflectionAgent::run`]'s loop and the
/// middleware form ([`crate::middleware::ReflectionMiddleware`], ADR 0025 phase 3e): one LLM
/// call with the structured-critique prompt, parsed via [`requests_revision`]. The middleware
/// uses this to ESCALATE a weak result to human review without re-running the agent (the full
/// revise loop stays in [`ReflectionAgent`] / the standalone reflection node — additive, not a
/// replacement).
pub async fn reflect_once(
    gateway: &Arc<dyn LlmGateway>,
    provider: LlmProvider,
    model: &str,
    draft: &str,
    score_threshold: f64,
    run_id: Option<&str>,
) -> Result<(bool, Vec<String>), LlmError> {
    let response = gateway
        .complete(LlmRequest {
            provider,
            model: model.to_owned(),
            messages: vec![LlmMessage::text(
                "user",
                format!("{CRITIQUE_INSTRUCTION}\n\nOutput to critique: {draft}"),
            )],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: run_id.map(str::to_owned),
        })
        .await?;
    Ok(requests_revision(&response.content, score_threshold))
}

/// What a reflection run produces: the accepted (or budget-exhausted) draft, how
/// many revision rounds ran, and the confidence the TS node would have set on
/// acceptance. `rounds` mirrors the TS `__reflectionCount` carried through the loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionResult {
    pub draft: String,
    pub rounds: usize,
    pub confidence: f64,
}

/// A reflection agent: critique a draft, revise while the critique flags a problem,
/// stop on acceptance or the round budget. Cheap to share behind an `Arc`.
pub struct ReflectionAgent {
    gateway: Arc<dyn LlmGateway>,
    provider: LlmProvider,
    model: String,
    max_reflections: usize,
    score_threshold: f64,
}

impl ReflectionAgent {
    /// An agent with the Rust defaults: Anthropic provider, the TS
    /// [`REFLECTION_MODEL`], [`DEFAULT_MAX_REFLECTIONS`] rounds, and
    /// [`DEFAULT_SCORE_THRESHOLD`] acceptance.
    pub fn new(gateway: Arc<dyn LlmGateway>) -> Self {
        ReflectionAgent {
            gateway,
            provider: LlmProvider::Anthropic,
            model: REFLECTION_MODEL.to_owned(),
            max_reflections: DEFAULT_MAX_REFLECTIONS,
            score_threshold: DEFAULT_SCORE_THRESHOLD,
        }
    }

    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_max_reflections(mut self, max_reflections: usize) -> Self {
        self.max_reflections = max_reflections;
        self
    }

    /// Accept a structured critique whose `score` is at least `score_threshold` (0..1).
    pub fn with_score_threshold(mut self, score_threshold: f64) -> Self {
        self.score_threshold = score_threshold;
        self
    }

    async fn critique(&self, draft: &str) -> Result<String, LlmError> {
        let response = self
            .gateway
            .complete(LlmRequest {
                provider: self.provider,
                model: self.model.clone(),
                // Mirrors the TS critique prompt: JSON instruction + the output to critique.
                messages: vec![LlmMessage::text(
                    "user",
                    format!("{CRITIQUE_INSTRUCTION}\n\nOutput to critique: {draft}"),
                )],
                system: None,
                tools: None,
                max_tokens: None,
                temperature: None,
                response_format: None,
                run_id: None,
            })
            .await?;
        Ok(response.content)
    }

    /// Critique `draft`, and while the critique flags a `"problem"`/`"retry"` and the
    /// budget allows, call `revise` to produce a fresh draft. `revise` is the Rust
    /// analogue of the TS node looping back to its previous node; it receives the
    /// current draft and the critique that rejected it. Stops on acceptance or after
    /// `max_reflections` revisions, then reports the final draft + round count.
    ///
    /// `base_confidence` is the incoming `confidence` channel value (TS default 0.5);
    /// on acceptance the result confidence is `min(1.0, base_confidence + 0.1)`,
    /// matching the TS bump.
    pub async fn run<F, Fut>(
        &self,
        initial_draft: impl Into<String>,
        base_confidence: f64,
        mut revise: F,
    ) -> Result<ReflectionResult, LlmError>
    where
        F: FnMut(&str, &str) -> Fut,
        Fut: std::future::Future<Output = Result<String, LlmError>>,
    {
        let mut draft = initial_draft.into();
        let mut rounds = 0usize;

        loop {
            let critique = self.critique(&draft).await?;
            // Structured verdict (ok || score >= threshold) when the reply is JSON; else the
            // legacy substring heuristic. `count < maxReflections` still bounds the loop.
            let (revise_needed, _issues) = requests_revision(&critique, self.score_threshold);
            if rounds < self.max_reflections && revise_needed {
                draft = revise(&draft, &critique).await?;
                rounds += 1;
                continue;
            }
            // Accept: bump confidence by 0.1, capped at 1.0 (TS `Math.min(1, …)`).
            return Ok(ReflectionResult {
                draft,
                rounds,
                confidence: (base_confidence + 0.1).min(1.0),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmUsage, MockAdapter};

    use super::*;

    fn text(content: &str) -> LlmResponse {
        LlmResponse {
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: Some("end_turn".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    fn gateway_with(responses: Vec<LlmResponse>) -> Arc<DefaultLlmGateway> {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            responses,
        )));
        Arc::new(gateway)
    }

    #[tokio::test]
    async fn accepts_early_when_the_critique_approves() {
        // First critique has no "problem"/"retry" → accept immediately, no revision.
        let agent = ReflectionAgent::new(gateway_with(vec![text("Looks great, ship it.")]));
        let revisions = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&revisions);
        let result = agent
            .run("draft v0", 0.5, |_draft, _critique| {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok("revised".to_owned())
                }
            })
            .await
            .unwrap();

        assert_eq!(revisions.load(Ordering::SeqCst), 0);
        assert_eq!(result.rounds, 0);
        assert_eq!(result.draft, "draft v0");
        // 0.5 + 0.1 bump on acceptance.
        assert!((result.confidence - 0.6).abs() < 1e-9);
    }

    #[tokio::test]
    async fn revises_once_then_accepts_on_clean_critique() {
        // Round 0: "problem" → revise. Round 1: clean → accept.
        let agent = ReflectionAgent::new(gateway_with(vec![
            text("There is a problem with the intro."),
            text("All good now."),
        ]));
        let result = agent
            .run("draft v0", 0.5, |draft, _critique| {
                let next = format!("{draft}+fix");
                async move { Ok(next) }
            })
            .await
            .unwrap();

        assert_eq!(result.rounds, 1);
        assert_eq!(result.draft, "draft v0+fix");
        assert!((result.confidence - 0.6).abs() < 1e-9);
    }

    #[tokio::test]
    async fn stops_at_max_rounds_when_the_critique_never_approves() {
        // Every critique flags "retry"; budget is the default 2 → exactly 2 revisions.
        let agent = ReflectionAgent::new(gateway_with(vec![text("please retry")]));
        let revisions = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&revisions);
        let result = agent
            .run("draft v0", 0.5, |_draft, _critique| {
                let counter = Arc::clone(&counter);
                async move {
                    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    Ok(format!("draft v{n}"))
                }
            })
            .await
            .unwrap();

        assert_eq!(revisions.load(Ordering::SeqCst), DEFAULT_MAX_REFLECTIONS);
        assert_eq!(result.rounds, DEFAULT_MAX_REFLECTIONS);
        assert_eq!(result.draft, "draft v2");
        // Budget-exhausted draft is still "accepted" with the same confidence bump.
        assert!((result.confidence - 0.6).abs() < 1e-9);
    }

    #[tokio::test]
    async fn confidence_bump_is_capped_at_one() {
        let agent = ReflectionAgent::new(gateway_with(vec![text("approved")]));
        let result = agent
            .run("draft", 0.95, |_d, _c| async { Ok(String::new()) })
            .await
            .unwrap();
        assert!((result.confidence - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn accepts_structured_score_at_or_above_threshold() {
        // ok:false but score 0.9 >= default 0.8 → accept, no revision.
        let agent = ReflectionAgent::new(gateway_with(vec![text(
            r#"{"ok": false, "score": 0.9, "issues": []}"#,
        )]));
        let revisions = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&revisions);
        let result = agent
            .run("draft", 0.5, |_d, _c| {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok("revised".to_owned())
                }
            })
            .await
            .unwrap();
        assert_eq!(revisions.load(Ordering::SeqCst), 0);
        assert_eq!(result.rounds, 0);
    }

    #[tokio::test]
    async fn revises_structured_below_threshold_then_accepts() {
        let agent = ReflectionAgent::new(gateway_with(vec![
            text(r#"{"ok": false, "score": 0.3, "issues": ["weak intro"]}"#),
            text(r#"{"ok": true, "score": 0.95, "issues": []}"#),
        ]));
        let result = agent
            .run("v0", 0.5, |d, _c| {
                let next = format!("{d}+fix");
                async move { Ok(next) }
            })
            .await
            .unwrap();
        assert_eq!(result.rounds, 1);
        assert_eq!(result.draft, "v0+fix");
    }

    #[tokio::test]
    async fn custom_score_threshold_is_honoured() {
        // score 0.5: default 0.8 would revise, but a 0.4 threshold accepts immediately.
        let agent = ReflectionAgent::new(gateway_with(vec![text(
            r#"{"ok": false, "score": 0.5, "issues": []}"#,
        )]))
        .with_score_threshold(0.4);
        let result = agent
            .run("d", 0.5, |_d, _c| async { Ok("r".to_owned()) })
            .await
            .unwrap();
        assert_eq!(result.rounds, 0);
    }

    #[test]
    fn parse_critique_handles_plain_wrapped_and_clamps() {
        assert_eq!(
            parse_critique(r#"{"ok":true,"score":0.9,"issues":[]}"#),
            Some(Critique {
                ok: true,
                score: 0.9,
                issues: vec![]
            })
        );
        let wrapped =
            parse_critique("Sure:\n```json\n{\"ok\":false,\"score\":0.4,\"issues\":[\"a\"]}\n```")
                .expect("wrapped JSON parses");
        assert_eq!(wrapped.issues, vec!["a".to_owned()]);
        assert!((parse_critique(r#"{"score": 1.7}"#).unwrap().score - 1.0).abs() < 1e-9);
        assert!(parse_critique("looks good to me").is_none());
        assert!(parse_critique(r#"{"unrelated": 1}"#).is_none());
    }
}
