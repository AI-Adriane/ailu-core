//! The unified agent middleware API (ADR 0025, deep-agent platform phase 3) — the
//! keystone composition surface for an agent's governance + efficiency behaviours.
//!
//! One [`AgentMiddleware`] trait with seven optional async hooks (all pass-through by
//! default), composed into one ordered [`MiddlewareStack`] the ReAct loop drives. The
//! existing scattered seams (PII redaction, prompt compression, terse output, context
//! trim, the approval gate, fs policy, the todos sink, reflection) fold onto these hooks
//! over phases 3b–3e; **phase 3a ships the trait + an empty default stack wired into the
//! loop with ZERO behaviour change** (an empty stack is a strict no-op).
//!
//! Governed-by-construction (the ADR 0013 bet): the stack separates a **governed** layer
//! (redaction, approval gate, fs policy — builder-injected, sealed) from a **user-tunable
//! efficiency** layer, so an ungoverned stack is unrepresentable.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use ailu_llm_gateway::{
    LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest, LlmResponse, PiiRedactor,
    PromptCompressor, ResponseFormat,
};
use serde_json::Value;

use crate::react::{AgentResult, ApprovalRequestItem};
use crate::reflection::reflect_once;
use crate::structured_output::{extract_first_json, validate_json};
use crate::tools::approval_key;

/// Control-flow signal a hook returns: continue the run, or stop it with a reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop { reason: String },
}

/// A `before_tool` decision (used from phase 3c, when the approval gate + fs policy fold
/// onto `before_tool`): allow the call (optionally overriding its input), deny it with a
/// reason (surfaced as an observation), or gate it for human approval.
#[derive(Clone, Debug)]
pub enum ToolControl {
    Allow { input_override: Option<Value> },
    Deny { reason: String },
    Gate(ApprovalRequestItem),
}

impl ToolControl {
    /// The pass-through default: allow, no override.
    pub fn allow() -> Self {
        ToolControl::Allow {
            input_override: None,
        }
    }
}

/// The tool call a `before_tool` hook inspects.
pub struct ToolCallCtx<'a> {
    pub name: &'a str,
    pub input: &'a Value,
    /// Whether the resolved tool is `requires_approval` (set by the loop).
    pub requires_approval: bool,
    /// Whether the resolved tool is `content_scoped` (ADR 0024 — guarded fs writes).
    pub content_scoped: bool,
}

/// A cheap, read-only snapshot of the loop state a hook may read. Built per hook-call,
/// never held across a loop mutation (avoids borrowing `conversation` while it mutates).
pub struct RunCtx<'a> {
    pub iteration: usize,
    pub approved_tool_names: &'a HashSet<String>,
    pub channels: &'a BTreeMap<String, Value>,
    /// The run (or subgraph child run, `{parent_run_id}:{node_id}`) this loop is executing
    /// for (ADR 0043). `None` for a caller that hasn't threaded one through yet — every
    /// existing construction predates this field and stays a no-op via `Default`-less
    /// explicit `None`.
    pub run_id: Option<&'a str>,
}

/// One composable agent middleware. Every hook has a pass-through default, so an impl
/// overrides only the lifecycle points it cares about. `async_trait` + `Arc<dyn …>`
/// (object-safe) matches the existing `PiiRedactor` / `PromptCompressor` seams.
#[async_trait::async_trait]
pub trait AgentMiddleware: Send + Sync {
    fn name(&self) -> &str {
        "middleware"
    }

    /// Once, after state injection, before the loop. May mutate the seed `conversation`
    /// (e.g. context-budget trim) or stop the run.
    async fn before_run(
        &self,
        _conversation: &mut Vec<LlmMessage>,
        _ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        Ok(Flow::Continue)
    }

    /// In-loop, just before `gateway.complete()`. **Fallible** — an `Err` short-circuits
    /// the run (the fail-closed path, e.g. redaction-block).
    async fn before_model(
        &self,
        request: LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        Ok(request)
    }

    /// After `complete()` returns, before the response is parsed.
    async fn after_model(
        &self,
        response: LlmResponse,
        _request: &LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmResponse, LlmError> {
        Ok(response)
    }

    /// Before a tool executes (fired from phase 3c). Returns an allow/deny/gate decision.
    async fn before_tool(
        &self,
        _call: &ToolCallCtx<'_>,
        _ctx: &RunCtx<'_>,
    ) -> Result<ToolControl, LlmError> {
        Ok(ToolControl::allow())
    }

    /// After a tool's handler returns, before its observation is recorded (fired from 3c).
    async fn after_tool(
        &self,
        _name: &str,
        _input: &Value,
        output: Value,
        _ctx: &RunCtx<'_>,
    ) -> Result<Value, LlmError> {
        Ok(output)
    }

    /// At each loop-turn end (loop-detection / budget / reflection trigger).
    fn on_iteration(&self, _index: usize, _content: &str, _ctx: &RunCtx<'_>) -> Flow {
        Flow::Continue
    }

    /// After the loop, before `AgentResult` is returned (finalize, reflection, metadata).
    async fn after_run(
        &self,
        _result: &mut AgentResult,
        _ctx: &RunCtx<'_>,
    ) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The ordered stack the loop drives. Two layers: **governed** (sealed, builder-injected
/// — redaction, fs policy) and **efficiency** (user-tunable — compression, terse,
/// context-budget). Request-path hooks fold governed→efficiency; response-path hooks fold
/// in reverse (onion semantics). The **approval gate is intrinsic** to [`before_tool`] (ADR
/// 0025 phase 3c) — evaluated before any installed middleware and impossible to omit, so an
/// empty stack still gates. Apart from that gate an empty stack is a strict no-op; only the
/// builder may populate `governed`.
///
/// [`before_tool`]: MiddlewareStack::before_tool
#[derive(Default, Clone)]
pub struct MiddlewareStack {
    governed: Vec<Arc<dyn AgentMiddleware>>,
    efficiency: Vec<Arc<dyn AgentMiddleware>>,
}

impl MiddlewareStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no middleware is installed — the hooks are then strict no-ops.
    pub fn is_empty(&self) -> bool {
        self.governed.is_empty() && self.efficiency.is_empty()
    }

    /// Number of installed EFFICIENCY middleware (the user-tunable layer). Lets callers assert
    /// what the SDK-resolved list produced independently of the always-present governed layer.
    pub fn efficiency_len(&self) -> usize {
        self.efficiency.len()
    }

    /// Append a GOVERNED middleware (builder-only; sealed/un-removable by users).
    pub fn push_governed(&mut self, middleware: Arc<dyn AgentMiddleware>) -> &mut Self {
        self.governed.push(middleware);
        self
    }

    /// Append a user-tunable EFFICIENCY middleware.
    pub fn push_efficiency(&mut self, middleware: Arc<dyn AgentMiddleware>) -> &mut Self {
        self.efficiency.push(middleware);
        self
    }

    /// Request-path order: governed first (outermost), then efficiency (inner).
    fn request_order(&self) -> impl Iterator<Item = &Arc<dyn AgentMiddleware>> {
        self.governed.iter().chain(self.efficiency.iter())
    }

    pub async fn before_run(
        &self,
        conversation: &mut Vec<LlmMessage>,
        ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        for middleware in self.request_order() {
            if let Flow::Stop { reason } = middleware.before_run(conversation, ctx).await? {
                return Ok(Flow::Stop { reason });
            }
        }
        Ok(Flow::Continue)
    }

    pub async fn before_model(
        &self,
        mut request: LlmRequest,
        ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        for middleware in self.request_order() {
            request = middleware.before_model(request, ctx).await?;
        }
        Ok(request)
    }

    pub async fn after_model(
        &self,
        mut response: LlmResponse,
        request: &LlmRequest,
        ctx: &RunCtx<'_>,
    ) -> Result<LlmResponse, LlmError> {
        // Response path is the reverse of the request path (onion).
        let reversed: Vec<&Arc<dyn AgentMiddleware>> = self.request_order().collect();
        for middleware in reversed.into_iter().rev() {
            response = middleware.after_model(response, request, ctx).await?;
        }
        Ok(response)
    }

    pub async fn before_tool(
        &self,
        call: &ToolCallCtx<'_>,
        ctx: &RunCtx<'_>,
    ) -> Result<ToolControl, LlmError> {
        // BUILT-IN governed approval gate (ADR 0025 phase 3c) — intrinsic to the stack,
        // evaluated before any installed middleware and impossible to omit (an empty stack
        // still gates). A `requires_approval` tool is gated unless this exact grant was
        // already approved by a human. The grant key is the tool name, OR — for a
        // content-scoped tool (a guarded fs write, ADR 0024 phase 2c) — the composite
        // "<name>#<sha256(input)>", so approving one call never unlocks a different
        // path/content (the over-grant guard). This is the SAME decision the ReAct loop
        // used to make inline; folding it here makes "no self-approval" a property of the
        // stack, not of one call site.
        if call.requires_approval {
            let key = approval_key(call.name, call.content_scoped, call.input);
            if !ctx.approved_tool_names.contains(&key) {
                return Ok(ToolControl::Gate(ApprovalRequestItem {
                    subject: format!("tool:{}", call.name),
                    reason: format!(
                        "Tool '{}' requires human approval before execution.",
                        call.name
                    ),
                    approval_key: call.content_scoped.then(|| key.clone()),
                    input: call.content_scoped.then(|| call.input.clone()),
                }));
            }
        }
        // Then installed before_tool middleware (fs policy, etc.); first non-Allow wins
        // (a deny/gate short-circuits execution).
        for middleware in self.request_order() {
            match middleware.before_tool(call, ctx).await? {
                ToolControl::Allow {
                    input_override: None,
                } => {}
                decision => return Ok(decision),
            }
        }
        Ok(ToolControl::allow())
    }

    pub async fn after_tool(
        &self,
        name: &str,
        input: &Value,
        mut output: Value,
        ctx: &RunCtx<'_>,
    ) -> Result<Value, LlmError> {
        let reversed: Vec<&Arc<dyn AgentMiddleware>> = self.request_order().collect();
        for middleware in reversed.into_iter().rev() {
            output = middleware.after_tool(name, input, output, ctx).await?;
        }
        Ok(output)
    }

    pub fn on_iteration(&self, index: usize, content: &str, ctx: &RunCtx<'_>) -> Flow {
        for middleware in self.request_order() {
            if let Flow::Stop { reason } = middleware.on_iteration(index, content, ctx) {
                return Flow::Stop { reason };
            }
        }
        Flow::Continue
    }

    pub async fn after_run(
        &self,
        result: &mut AgentResult,
        ctx: &RunCtx<'_>,
    ) -> Result<(), LlmError> {
        let reversed: Vec<&Arc<dyn AgentMiddleware>> = self.request_order().collect();
        for middleware in reversed.into_iter().rev() {
            middleware.after_run(result, ctx).await?;
        }
        Ok(())
    }
}

// ── Built-in middleware (folded from the gateway seams, ADR 0025 phase 3b) ──────────────

/// GOVERNED — PII redaction (ADR 0008) as before/after-model hooks. `before_model` scrubs
/// the request (**fail-closed**: an `Err`, e.g. `PiiBlocked`, short-circuits the run);
/// `after_model` hydrates the response. Reuses the existing [`PiiRedactor`] (and its HTTP
/// impl) verbatim — only the composition moves from a gateway wrapper to the stack.
pub struct RedactMiddleware {
    redactor: Arc<dyn PiiRedactor>,
}

impl RedactMiddleware {
    pub fn new(redactor: Arc<dyn PiiRedactor>) -> Self {
        Self { redactor }
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for RedactMiddleware {
    fn name(&self) -> &str {
        "redact"
    }
    async fn before_model(
        &self,
        request: LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        self.redactor.redact_request(request).await
    }
    async fn after_model(
        &self,
        response: LlmResponse,
        _request: &LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmResponse, LlmError> {
        Ok(self.redactor.hydrate_response(response).await)
    }
}

/// EFFICIENCY — prompt compression (ADR 0014) as a before-model hook: shrinks `user`-role
/// message content. **Fail-open** (the compressor returns the text unchanged on any
/// error; this hook never `Err`s). Reuses the existing [`PromptCompressor`] verbatim.
pub struct CompressMiddleware {
    compressor: Arc<dyn PromptCompressor>,
}

impl CompressMiddleware {
    pub fn new(compressor: Arc<dyn PromptCompressor>) -> Self {
        Self { compressor }
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for CompressMiddleware {
    fn name(&self) -> &str {
        "compress"
    }
    async fn before_model(
        &self,
        mut request: LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        for message in request.messages.iter_mut() {
            if message.role == "user" {
                let text = std::mem::take(&mut message.content);
                message.content = self.compressor.compress(text).await;
            }
        }
        Ok(request)
    }
}

/// The terse-output directive (ADR 0014) [`TerseMiddleware`] appends to the system prompt.
const TERSE_SUFFIX: &str = " Respond in a terse, telegraphic style: sentence fragments, no \
    filler, no pleasantries. Preserve ALL technical substance, numbers, code and exact values.";

/// EFFICIENCY — terse output (ADR 0014) as a before-model hook: appends [`TERSE_SUFFIX`] to
/// the request's system prompt so the model answers compactly (cuts output tokens on prose;
/// lossy — the SDK only sets it for prose stages). Idempotent per request: the loop rebuilds
/// the request from the agent's bare system prompt each iteration, so exactly one suffix is
/// appended each call. Folds the former flat `output_style:"terse"` bridge knob (ADR 0025 3d).
pub struct TerseMiddleware;

#[async_trait::async_trait]
impl AgentMiddleware for TerseMiddleware {
    fn name(&self) -> &str {
        "terse"
    }
    async fn before_model(
        &self,
        mut request: LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        request.system = Some(match request.system {
            Some(system) => format!("{system}{TERSE_SUFFIX}"),
            None => TERSE_SUFFIX.trim().to_owned(),
        });
        Ok(request)
    }
}

/// EFFICIENCY — context-budget trim (ADR 0014) as a before-run hook: caps the agent's seed
/// message (the injected `Input/State`) to `chars` characters so an unbounded channel map is
/// not re-fed to the model. Truncates on a char boundary and marks the cut with `…`. Folds
/// the former flat `context_budget` agent knob into a composable middleware (ADR 0025 3d).
pub struct ContextBudgetMiddleware {
    chars: usize,
}

impl ContextBudgetMiddleware {
    pub fn new(chars: usize) -> Self {
        Self { chars }
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for ContextBudgetMiddleware {
    fn name(&self) -> &str {
        "context-budget"
    }
    async fn before_run(
        &self,
        conversation: &mut Vec<LlmMessage>,
        _ctx: &RunCtx<'_>,
    ) -> Result<Flow, LlmError> {
        if let Some(first) = conversation.first_mut() {
            if first.content.chars().count() > self.chars {
                let truncated: String = first.content.chars().take(self.chars).collect();
                first.content = truncated + "…";
            }
        }
        Ok(Flow::Continue)
    }
}

/// EFFICIENCY (opt-in quality signal) — reflection as an after-run hook (ADR 0025 phase 3e).
/// Runs ONE self-critique over the agent's reasoning ([`reflect_once`]); when the critique
/// rejects the output it ANNOTATES the reasoning with `reflection:needs_review:<issues>`.
///
/// It deliberately does **NOT** set `requires_human_review`: that field drives the
/// approval-suspend gate (`node.rs`), and since a suspended agent node re-runs the whole agent
/// (and so re-critiques) on resume, a quality escalation through that flag would re-suspend
/// forever with no approval to grant. Quality escalation is therefore a graph-level concern — a
/// conditional edge can route on the `reflection:needs_review` marker into a human-gate node —
/// not an agent-internal flag. **Additive**: the full critique→revise loop stays the standalone
/// [`crate::reflection::ReflectionAgent`] / reflection node (ADR 0025: add the middleware form
/// WITHOUT removing the node). **Fail-open**: a critique-call error never fails the run.
pub struct ReflectionMiddleware {
    gateway: Arc<dyn LlmGateway>,
    provider: LlmProvider,
    model: String,
    score_threshold: f64,
}

impl ReflectionMiddleware {
    pub fn new(
        gateway: Arc<dyn LlmGateway>,
        provider: LlmProvider,
        model: impl Into<String>,
        score_threshold: f64,
    ) -> Self {
        Self {
            gateway,
            provider,
            model: model.into(),
            score_threshold,
        }
    }
}

#[async_trait::async_trait]
impl AgentMiddleware for ReflectionMiddleware {
    fn name(&self) -> &str {
        "reflection"
    }
    async fn after_run(&self, result: &mut AgentResult, ctx: &RunCtx<'_>) -> Result<(), LlmError> {
        // Fail-open: a critique-call failure must not sink an otherwise-good run. On a rejecting
        // critique, annotate the reasoning (a graph may route on the marker) — but never touch
        // `requires_human_review` (see the type doc: it would re-suspend forever on resume).
        if let Ok((revise_needed, issues)) = reflect_once(
            &self.gateway,
            self.provider,
            &self.model,
            &result.reasoning,
            self.score_threshold,
            ctx.run_id,
        )
        .await
        {
            if revise_needed {
                let note = if issues.is_empty() {
                    "reflection:needs_review".to_owned()
                } else {
                    format!("reflection:needs_review:{}", issues.join("; "))
                };
                result.reasoning = format!("{}\n{note}", result.reasoning);
            }
        }
        Ok(())
    }
}

/// Constrain an agent's output to a JSON schema (ADR 0029 phase 8 — efficiency layer,
/// user-installable). `before_model` sets the request's `response_format` (each adapter
/// fans it out to its provider's native route — OpenAI `response_format`, Anthropic
/// forced tool, Gemini `responseSchema`); `after_model` extracts + validates the JSON
/// against the schema (the in-engine validation floor), with a bounded deterministic
/// retry, then normalizes the response into a final answer; `after_run` attaches the
/// validated value to `AgentResult.structured_output`.
///
/// Gate-safety: this lives in the efficiency layer and the approval gate is intrinsic to
/// `MiddlewareStack::before_tool` (phase 3c) — so it cannot route around governance.
pub struct StructuredOutputMiddleware {
    gateway: Arc<dyn LlmGateway>,
    schema_name: String,
    schema: Value,
    strict: bool,
    /// `false` (required): invalid output after the retry budget fails closed with a typed
    /// error. `true` (lenient): fall back to the raw response (no structured value attached).
    lenient: bool,
    /// Extra deterministic re-prompts on invalid output (no temperature drift). `0` = none.
    retry_cap: usize,
    /// Interior-mutable stash so `after_run` can attach the value validated in `after_model`.
    last_valid: Mutex<Option<Value>>,
}

impl StructuredOutputMiddleware {
    pub fn new(
        gateway: Arc<dyn LlmGateway>,
        schema_name: impl Into<String>,
        schema: Value,
        strict: bool,
        lenient: bool,
        retry_cap: usize,
    ) -> Self {
        Self {
            gateway,
            schema_name: schema_name.into(),
            schema,
            strict,
            lenient,
            retry_cap,
            last_valid: Mutex::new(None),
        }
    }

    /// The candidate JSON: the Anthropic forced-tool call's input if present, else the
    /// first JSON value embedded in the content (OpenAI / Gemini native path).
    fn extract_candidate(&self, response: &LlmResponse) -> Option<Value> {
        if let Some(calls) = &response.tool_calls {
            if let Some(call) = calls.iter().find(|c| c.name == self.schema_name) {
                return Some(call.input.clone());
            }
        }
        extract_first_json(&response.content)
    }

    /// A deterministic corrective re-prompt: the same request (schema still attached) plus a
    /// user message naming the problem. Temperature is untouched, so replay stays stable.
    fn corrective_request(&self, base: &LlmRequest, problem: &str) -> LlmRequest {
        let mut req = base.clone();
        req.messages.push(LlmMessage::text(
            "user",
            format!(
                "Your previous response was rejected: {problem}. Respond with ONLY a JSON value \
                 matching the required schema — no prose, no code fences."
            ),
        ));
        req
    }
}

/// Rewrite a validated response into a clean final answer so the loop terminates on it:
/// the JSON becomes the content and any (synthetic) tool call is dropped.
fn finalize_structured(mut response: LlmResponse, value: &Value) -> LlmResponse {
    response.content = serde_json::to_string(value).unwrap_or_default();
    response.tool_calls = None;
    response
}

#[async_trait::async_trait]
impl AgentMiddleware for StructuredOutputMiddleware {
    fn name(&self) -> &str {
        "structuredOutput"
    }

    async fn before_model(
        &self,
        mut request: LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmRequest, LlmError> {
        request.response_format = Some(ResponseFormat::JsonSchema {
            name: self.schema_name.clone(),
            schema: self.schema.clone(),
            strict: self.strict,
        });
        // A light reminder helps providers without native schema decoding comply.
        let reminder = format!(
            "You must respond with a single JSON value matching the '{}' schema. \
             No prose, no code fences.",
            self.schema_name
        );
        request.system = Some(match request.system.take() {
            Some(s) if !s.is_empty() => format!("{s}\n\n{reminder}"),
            _ => reminder,
        });
        Ok(request)
    }

    async fn after_model(
        &self,
        response: LlmResponse,
        request: &LlmRequest,
        _ctx: &RunCtx<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let mut current = response;
        let mut attempt = 0usize;
        loop {
            let candidate = self.extract_candidate(&current);
            let problem: Option<String> = match &candidate {
                Some(value) => match validate_json(&self.schema, value) {
                    Ok(()) => {
                        *self.last_valid.lock().expect("structured output mutex") =
                            Some(value.clone());
                        return Ok(finalize_structured(current, value));
                    }
                    Err(msg) => Some(format!("the JSON did not match the schema: {msg}")),
                },
                None => Some("no JSON value was found in the response".to_owned()),
            };

            if attempt >= self.retry_cap {
                // Budget exhausted. Lenient → fail-open (raw response, no value attached);
                // required → fail-closed with a typed error (surfaced as channel data).
                if self.lenient {
                    return Ok(current);
                }
                return Err(LlmError::StructuredOutputInvalid(
                    problem.unwrap_or_else(|| "invalid output".to_owned()),
                ));
            }
            attempt += 1;
            let retry = self.corrective_request(request, problem.as_deref().unwrap_or(""));
            current = self.gateway.complete(retry).await?;
        }
    }

    async fn after_run(&self, result: &mut AgentResult, _ctx: &RunCtx<'_>) -> Result<(), LlmError> {
        if let Some(value) = self
            .last_valid
            .lock()
            .expect("structured output mutex")
            .take()
        {
            result.structured_output = Some(value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_stack_is_a_no_op() {
        let stack = MiddlewareStack::new();
        assert!(stack.is_empty());
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &approved,
            channels: &channels,
            run_id: None,
        };
        // before_run does not stop, does not mutate.
        let mut conversation = vec![LlmMessage::text("user", "hi")];
        assert_eq!(
            stack.before_run(&mut conversation, &ctx).await.unwrap(),
            Flow::Continue
        );
        assert_eq!(conversation.len(), 1);
        // before_tool allows by default.
        let call = ToolCallCtx {
            name: "t",
            input: &Value::Null,
            requires_approval: false,
            content_scoped: false,
        };
        assert!(matches!(
            stack.before_tool(&call, &ctx).await.unwrap(),
            ToolControl::Allow {
                input_override: None
            }
        ));
        assert_eq!(stack.on_iteration(0, "x", &ctx), Flow::Continue);
    }

    /// A recorder asserting the onion order: request-path governed→efficiency,
    /// response-path reversed. This locks the redact→compress order (ADR 0025 3b).
    struct Recorder {
        label: String,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl AgentMiddleware for Recorder {
        async fn before_model(
            &self,
            request: LlmRequest,
            _ctx: &RunCtx<'_>,
        ) -> Result<LlmRequest, LlmError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("before:{}", self.label));
            Ok(request)
        }
        async fn after_model(
            &self,
            response: LlmResponse,
            _request: &LlmRequest,
            _ctx: &RunCtx<'_>,
        ) -> Result<LlmResponse, LlmError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("after:{}", self.label));
            Ok(response)
        }
    }

    #[tokio::test]
    async fn onion_order_governed_outermost_then_reversed_on_response() {
        use ailu_llm_gateway::{LlmProvider, LlmUsage};

        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut stack = MiddlewareStack::new();
        // governed = redact (outermost); efficiency = compress (inner).
        stack.push_governed(Arc::new(Recorder {
            label: "redact".to_owned(),
            log: log.clone(),
        }));
        stack.push_efficiency(Arc::new(Recorder {
            label: "compress".to_owned(),
            log: log.clone(),
        }));

        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &approved,
            channels: &channels,
            run_id: None,
        };
        let request = LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "m".to_owned(),
            messages: vec![],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        };
        let request = stack.before_model(request, &ctx).await.unwrap();
        let response = LlmResponse {
            content: String::new(),
            tool_calls: None,
            stop_reason: None,
            usage: LlmUsage::default(),
            model: "m".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        };
        let _ = stack.after_model(response, &request, &ctx).await.unwrap();

        // Request: governed (redact) before efficiency (compress). Response: reversed.
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "before:redact",
                "before:compress",
                "after:compress",
                "after:redact"
            ]
        );
    }

    #[tokio::test]
    async fn intrinsic_approval_gate_fires_even_on_an_empty_stack() {
        // The gate is built into the stack (ADR 0025 3c), so an EMPTY stack still gates a
        // `requires_approval` tool — "no self-approval" is a property of the stack itself,
        // not of any installed middleware that a caller could forget to add.
        let stack = MiddlewareStack::new();
        assert!(stack.is_empty());
        let channels = BTreeMap::new();
        let input = serde_json::json!({ "path": "/x" });
        let call = ToolCallCtx {
            name: "writeFile",
            input: &input,
            requires_approval: true,
            content_scoped: false,
        };

        // Not granted → gated.
        let none = HashSet::new();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &none,
            channels: &channels,
            run_id: None,
        };
        assert!(matches!(
            stack.before_tool(&call, &ctx).await.unwrap(),
            ToolControl::Gate(_)
        ));

        // Granted by name → allowed.
        let granted: HashSet<String> = ["writeFile".to_owned()].into_iter().collect();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &granted,
            channels: &channels,
            run_id: None,
        };
        assert!(matches!(
            stack.before_tool(&call, &ctx).await.unwrap(),
            ToolControl::Allow {
                input_override: None
            }
        ));

        // A non-gated tool is always allowed.
        let plain = ToolCallCtx {
            name: "search",
            input: &input,
            requires_approval: false,
            content_scoped: false,
        };
        assert!(matches!(
            stack.before_tool(&plain, &ctx).await.unwrap(),
            ToolControl::Allow { .. }
        ));
    }

    #[tokio::test]
    async fn intrinsic_gate_content_scoped_pins_the_grant_to_the_exact_input() {
        // The over-grant guard survives the fold: a content-scoped gate emits the composite
        // "<name>#<hash>" key + echoes the input, and granting it unlocks only THAT call.
        let stack = MiddlewareStack::new();
        let channels = BTreeMap::new();
        let input = serde_json::json!({ "path": "/secret", "content": "x" });
        let call = ToolCallCtx {
            name: "writeFile",
            input: &input,
            requires_approval: true,
            content_scoped: true,
        };

        let none = HashSet::new();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &none,
            channels: &channels,
            run_id: None,
        };
        let key = match stack.before_tool(&call, &ctx).await.unwrap() {
            ToolControl::Gate(item) => {
                assert_eq!(item.input.as_ref(), Some(&input));
                item.approval_key
                    .expect("a content-scoped gate carries the composite approval key")
            }
            other => panic!("expected Gate, got {other:?}"),
        };
        assert!(key.starts_with("writeFile#"));

        // Granting that exact composite key unlocks this call …
        let granted: HashSet<String> = [key].into_iter().collect();
        let ctx = RunCtx {
            iteration: 0,
            approved_tool_names: &granted,
            channels: &channels,
            run_id: None,
        };
        assert!(matches!(
            stack.before_tool(&call, &ctx).await.unwrap(),
            ToolControl::Allow { .. }
        ));

        // … but a different input re-gates (no over-grant across paths/contents).
        let other_input = serde_json::json!({ "path": "/other", "content": "y" });
        let other_call = ToolCallCtx {
            name: "writeFile",
            input: &other_input,
            requires_approval: true,
            content_scoped: true,
        };
        assert!(matches!(
            stack.before_tool(&other_call, &ctx).await.unwrap(),
            ToolControl::Gate(_)
        ));
    }

    fn empty_ctx<'a>(
        approved: &'a HashSet<String>,
        channels: &'a BTreeMap<String, Value>,
    ) -> RunCtx<'a> {
        RunCtx {
            iteration: 0,
            approved_tool_names: approved,
            channels,
            run_id: None,
        }
    }

    fn bare_request(system: Option<&str>) -> LlmRequest {
        use ailu_llm_gateway::LlmProvider;
        LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "m".to_owned(),
            messages: vec![],
            system: system.map(str::to_owned),
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        }
    }

    #[tokio::test]
    async fn terse_middleware_appends_the_directive_idempotently_per_request() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let terse = TerseMiddleware;

        // With a base system → suffix appended after it.
        let out = terse
            .before_model(bare_request(Some("Be helpful.")), &ctx)
            .await
            .unwrap();
        let system = out.system.expect("system set");
        assert!(system.starts_with("Be helpful."));
        assert!(system.contains("terse, telegraphic"));

        // Without a base system → the trimmed directive becomes the system.
        let out = terse.before_model(bare_request(None), &ctx).await.unwrap();
        let system = out.system.expect("system set");
        assert!(system.starts_with("Respond in a terse"));

        // Per-request idempotency: re-running on a fresh (bare) request appends exactly once
        // (the loop never persists the suffix back onto the agent's system prompt).
        let out = terse
            .before_model(bare_request(Some("Be helpful.")), &ctx)
            .await
            .unwrap();
        assert_eq!(out.system.unwrap().matches("terse, telegraphic").count(), 1);
    }

    #[tokio::test]
    async fn context_budget_middleware_trims_only_when_over_budget() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = ContextBudgetMiddleware::new(10);

        // Over budget → truncated to `chars` + ellipsis marker.
        let mut conversation = vec![LlmMessage::text("user", "0123456789ABCDEF")];
        assert_eq!(
            mw.before_run(&mut conversation, &ctx).await.unwrap(),
            Flow::Continue
        );
        assert_eq!(conversation[0].content, "0123456789…");

        // Within budget → untouched (no ellipsis).
        let mut conversation = vec![LlmMessage::text("user", "short")];
        mw.before_run(&mut conversation, &ctx).await.unwrap();
        assert_eq!(conversation[0].content, "short");

        // Empty conversation → no panic.
        let mut empty: Vec<LlmMessage> = vec![];
        mw.before_run(&mut empty, &ctx).await.unwrap();
        assert!(empty.is_empty());
    }

    fn result_with(reasoning: &str) -> AgentResult {
        AgentResult {
            reasoning: reasoning.to_owned(),
            approval_requests: vec![],
            requires_human_review: false,
            todos: None,
            usage: None,
            structured_output: None,
            memory_writes: None,
        }
    }

    fn gateway_returning(content: &str) -> Arc<ailu_llm_gateway::DefaultLlmGateway> {
        use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmUsage, MockAdapter};
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(MockAdapter::new(
            LlmProvider::Anthropic,
            vec![LlmResponse {
                content: content.to_owned(),
                tool_calls: None,
                stop_reason: Some("end_turn".to_owned()),
                usage: LlmUsage::default(),
                model: "m".to_owned(),
                provider: LlmProvider::Anthropic,
                content_blocks: None,
            }],
        )));
        Arc::new(gateway)
    }

    #[tokio::test]
    async fn reflection_middleware_annotates_a_weak_result_without_forcing_suspend() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        // A rejecting structured critique (score 0.2 < 0.8) → annotate the reasoning with the
        // issue, but DO NOT set requires_human_review (that would re-suspend forever on resume).
        let mw = ReflectionMiddleware::new(
            gateway_returning(r#"{"ok": false, "score": 0.2, "issues": ["weak intro"]}"#),
            LlmProvider::Anthropic,
            "m",
            0.8,
        );
        let mut result = result_with("the agent's answer");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert!(!result.requires_human_review);
        assert!(result.reasoning.contains("reflection:needs_review"));
        assert!(result.reasoning.contains("weak intro"));
    }

    // --- StructuredOutputMiddleware (ADR 0029 phase 8) ---

    fn verdict_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"]
        })
    }

    fn resp(content: &str) -> LlmResponse {
        use ailu_llm_gateway::LlmUsage;
        LlmResponse {
            content: content.to_owned(),
            tool_calls: None,
            stop_reason: None,
            usage: LlmUsage::default(),
            model: "m".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        }
    }

    #[tokio::test]
    async fn structured_output_validates_normalizes_and_attaches() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = StructuredOutputMiddleware::new(
            gateway_returning("unused"),
            "Verdict",
            verdict_schema(),
            true,
            false,
            0,
        );

        // Valid JSON embedded in prose → extracted, validated, normalized to clean JSON.
        let out = mw
            .after_model(
                resp(r#"Here: {"ok": true} done"#),
                &bare_request(None),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, r#"{"ok":true}"#);
        assert!(out.tool_calls.is_none(), "loop must see a final answer");

        // after_run attaches the validated value.
        let mut result = result_with("x");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert_eq!(
            result.structured_output,
            Some(serde_json::json!({ "ok": true }))
        );
    }

    #[tokio::test]
    async fn structured_output_required_fails_closed_on_invalid() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = StructuredOutputMiddleware::new(
            gateway_returning("unused"),
            "Verdict",
            verdict_schema(),
            true,
            false, // required
            0,     // no retry
        );
        // Wrong type for `ok` → schema violation, no retry budget → typed error.
        let err = mw
            .after_model(resp(r#"{"ok": "nope"}"#), &bare_request(None), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::StructuredOutputInvalid(_)));
    }

    #[tokio::test]
    async fn structured_output_lenient_fails_open_to_raw_text() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = StructuredOutputMiddleware::new(
            gateway_returning("unused"),
            "Verdict",
            verdict_schema(),
            false,
            true, // lenient
            0,
        );
        let out = mw
            .after_model(resp("not json at all"), &bare_request(None), &ctx)
            .await
            .unwrap();
        assert_eq!(
            out.content, "not json at all",
            "raw response passes through"
        );
        let mut result = result_with("x");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert_eq!(
            result.structured_output, None,
            "nothing attached when lenient-open"
        );
    }

    #[tokio::test]
    async fn structured_output_recovers_within_the_retry_budget() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        // The corrective re-prompt's gateway returns valid JSON.
        let mw = StructuredOutputMiddleware::new(
            gateway_returning(r#"{"ok": true}"#),
            "Verdict",
            verdict_schema(),
            true,
            false,
            1, // one retry allowed
        );
        // First model response is invalid → one corrective call → valid.
        let out = mw
            .after_model(resp("garbage"), &bare_request(None), &ctx)
            .await
            .unwrap();
        assert_eq!(out.content, r#"{"ok":true}"#);
        let mut result = result_with("x");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert_eq!(
            result.structured_output,
            Some(serde_json::json!({ "ok": true }))
        );
    }

    #[tokio::test]
    async fn structured_output_extracts_anthropic_forced_tool_call() {
        use ailu_llm_gateway::{LlmToolCall, LlmUsage};
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = StructuredOutputMiddleware::new(
            gateway_returning("unused"),
            "Verdict",
            verdict_schema(),
            true,
            false,
            0,
        );
        // Anthropic forced-tool: the JSON arrives as the synthetic tool call's input.
        let response = LlmResponse {
            content: String::new(),
            tool_calls: Some(vec![LlmToolCall {
                id: "1".to_owned(),
                name: "Verdict".to_owned(),
                input: serde_json::json!({ "ok": false }),
            }]),
            stop_reason: Some("tool_use".to_owned()),
            usage: LlmUsage::default(),
            model: "m".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        };
        let out = mw
            .after_model(response, &bare_request(None), &ctx)
            .await
            .unwrap();
        assert_eq!(out.content, r#"{"ok":false}"#);
        assert!(out.tool_calls.is_none());
    }

    #[tokio::test]
    async fn structured_output_before_model_sets_response_format_and_reminder() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = StructuredOutputMiddleware::new(
            gateway_returning("unused"),
            "Verdict",
            verdict_schema(),
            true,
            false,
            0,
        );
        let out = mw
            .before_model(bare_request(Some("Be helpful.")), &ctx)
            .await
            .unwrap();
        match out.response_format {
            Some(ResponseFormat::JsonSchema { name, strict, .. }) => {
                assert_eq!(name, "Verdict");
                assert!(strict);
            }
            None => panic!("response_format not set"),
        }
        let system = out.system.expect("system set");
        assert!(system.starts_with("Be helpful."));
        assert!(system.contains("Verdict"));
    }

    #[tokio::test]
    async fn reflection_middleware_leaves_an_accepted_result_untouched() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        let mw = ReflectionMiddleware::new(
            gateway_returning(r#"{"ok": true, "score": 0.95, "issues": []}"#),
            LlmProvider::Anthropic,
            "m",
            0.8,
        );
        let mut result = result_with("a solid answer");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert!(!result.requires_human_review);
        assert_eq!(result.reasoning, "a solid answer");
    }

    #[tokio::test]
    async fn reflection_middleware_is_fail_open_on_a_critique_error() {
        let approved = HashSet::new();
        let channels = BTreeMap::new();
        let ctx = empty_ctx(&approved, &channels);
        // A gateway with no adapter registered → the critique call errors → fail-open: the
        // result is returned unchanged rather than failing the run.
        let gateway = Arc::new(ailu_llm_gateway::DefaultLlmGateway::new());
        let mw = ReflectionMiddleware::new(gateway, LlmProvider::Anthropic, "m", 0.8);
        let mut result = result_with("an answer");
        mw.after_run(&mut result, &ctx).await.unwrap();
        assert!(!result.requires_human_review);
        assert_eq!(result.reasoning, "an answer");
    }
}
