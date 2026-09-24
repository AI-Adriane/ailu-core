//! The ReAct loop — the Rust port of `@ailu/agents-core`'s `ReActAgent`.
//!
//! Mirrors the TS behavioural contract:
//! - native `tool_calls` on a response take precedence over the text protocol;
//! - the text protocol understands `FINAL: <answer>` (stop) and
//!   `ACTION: <tool> <optional json>` (call a tool);
//! - a `requires_approval` tool is **never** executed by the agent itself: unless
//!   its name is in `approved_tool_names`, an approval request is recorded and the
//!   loop stops — no self-approval, ever.
//!
//! One adaptation: tool results are fed back as `observation:<json>` user messages on
//! both paths, the way the TS text protocol does. The seed user message may be multimodal
//! when an `input_blocks_channel` is bound (ADR 0030 phase 9e); tool-result turns stay text.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ailu_llm_gateway::{
    ContentBlock, LlmError, LlmGateway, LlmMessage, LlmProvider, LlmRequest, LlmToolDef, LlmUsage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memory_tools::{MemoryWrite, REMEMBER_MEMORY_TOOL};
use crate::middleware::{Flow, MiddlewareStack, RunCtx, ToolCallCtx, ToolControl};
use crate::todos::{TodoItem, WRITE_TODOS_TOOL};
use crate::tools::InMemoryToolRegistry;

/// Default model, matching the TS `DEFAULT_MODEL`.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// Default iteration budget, matching the TS default.
pub const DEFAULT_MAX_ITERATIONS: usize = 6;

/// Sink for observational per-token deltas (ADR 0033, phase 13). Implemented in `bindings`
/// to forward each delta to JS as a `RunEvent::TokenDelta` over the napi `on_event` TSFN —
/// crucially **bypassing the runtime `EventBus`**, so token deltas never enter a checkpoint
/// or the durable event journal (durability ≠ observability). Defined here, not in
/// `graph-runtime`, so `agents-core` gains no dependency on the runtime crate.
///
/// It is purely observational: emitting deltas must not affect the agent's result. The
/// authoritative response is always the one `gateway.stream()` returns (byte-identical to
/// `complete()`); deltas are a side view of the same generation.
pub trait EventSink: Send + Sync {
    /// Emit one token delta. `spawn_id` tags which `mapAgents` sub-agent produced it
    /// (`None` for a top-level agent node). `message_id` groups all deltas of one agent
    /// turn, so a consumer concatenates them into a single streamed message.
    fn token_delta(&self, spawn_id: Option<u32>, message_id: &str, delta: &str);
}

/// One pending approval. `subject` is `"tool:<name>"`, exactly like the TS shape
/// (`{ description: "tool:<name>" }`) flattened to its description string.
///
/// For a **content-scoped** tool (ADR 0024 phase 2c — the guarded fs writes), the
/// approval is pinned to the exact call: `approval_key` is the composite
/// `"<name>#<sha256(input)>"` that must be granted to unlock THIS write (a different
/// path/content hashes differently and re-gates — no over-grant), and `input` carries
/// the gated tool input so a reviewer sees the path + content. Both are `None` for an
/// ordinary name-only gate (grant = the tool name). `Eq` is dropped because `input`
/// holds a `serde_json::Value` (same reason as `LlmMessage`); `PartialEq` is kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestItem {
    pub subject: String,
    pub reason: String,
    /// Content-scoped pin (`"<name>#<hash>"`) that must be granted to unlock this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_key: Option<String>,
    /// The gated tool input, surfaced so the control plane can show what is approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

/// What an agent run produces — wire-compatible (camelCase) with the TS
/// `AgentResult` subset: `reasoning`, `approvalRequests`, `requiresHumanReview`.
/// `Eq` is dropped (an `ApprovalRequestItem.input` may hold a non-`Eq`
/// `serde_json::Value`); `PartialEq` is retained for tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResult {
    pub reasoning: String,
    pub approval_requests: Vec<ApprovalRequestItem>,
    pub requires_human_review: bool,
    /// The authoritative todo list from the most recent `writeTodos` call this run,
    /// if any (ADR 0022/0023, phase 1). Additive + optional: omitted from the wire
    /// when absent (`skip_serializing_if`), so existing `AgentResult` consumers stay
    /// compatible. The node handler persists it into the durable todos channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todos: Option<Vec<TodoItem>>,
    /// Token usage summed across this run's LLM calls (ADR 0028 phase 7a — observability /
    /// cost). Additive + optional (`skip_serializing_if`), so existing consumers stay
    /// compatible; the control plane maps it to cost and to span/trace attributes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
    /// The validated structured output (ADR 0029 phase 8), when the agent ran with a
    /// `structuredOutput` middleware: the parsed JSON value that conformed to the schema.
    /// Additive + optional (`skip_serializing_if`); `None` when no schema was requested
    /// or (in lenient mode) the output never validated. The `StructuredOutputMiddleware`
    /// attaches it in `after_run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    /// Durable memory write intents from this run's `rememberMemory` tool calls (ADR 0045 Stage 1b).
    /// Additive + optional (`skip_serializing_if`). The node handler patches them into the reserved
    /// `__memoryWrites` channel so the control plane persists them durably (Neo4j supersede/forget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_writes: Option<Vec<MemoryWrite>>,
}

/// Outcome of the shared tool-execution path (native and `ACTION:` calls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolOutcome {
    /// The tool ran; its observation went into the trace and conversation.
    Executed,
    /// The tool is approval-gated and not granted — the run must stop here.
    Approval,
    /// No such tool; recorded in the trace, the loop moves on.
    NotFound,
}

/// A ReAct agent over the async LLM gateway: think → act (tool) → observe, until
/// `FINAL:` or the iteration budget. Cheap to share behind an `Arc` — see
/// [`crate::node::agent_node_handler`] for the graph-runtime integration.
pub struct ReActAgent {
    pub name: String,
    pub description: String,
    gateway: Arc<dyn LlmGateway>,
    tools: Option<Arc<InMemoryToolRegistry>>,
    provider: LlmProvider,
    model: String,
    system: Option<String>,
    max_iterations: usize,
    /// ADR 0025: the agent middleware stack the loop drives. The approval gate is
    /// intrinsic (see `MiddlewareStack::before_tool`); efficiency hooks (compress, terse,
    /// context-budget trim) fold on via the stack rather than as flat agent knobs (3d).
    middleware: MiddlewareStack,
    /// ADR 0030 phase 9 (9e): the channel whose value carries this run's multimodal input
    /// (a `Vec<ContentBlock>`). When set and present, the seed user message becomes a
    /// multimodal `with_blocks` message (text Input/State digest + the media blocks), and
    /// the channel is excluded from the stringified State dump so binary bytes are not
    /// re-fed as text. `None` → text-only seed (unchanged).
    input_blocks_channel: Option<String>,
    /// ADR 0033 phase 13: an optional observational token-delta sink. When `Some`, the loop
    /// drives `gateway.stream()` and emits each delta; when `None` it calls `gateway.complete()`
    /// and the path is byte-identical to before. Opt-in by construction.
    event_sink: Option<Arc<dyn EventSink>>,
}

impl ReActAgent {
    /// An agent with the TS defaults: Anthropic, [`DEFAULT_MODEL`], no tools, no
    /// system prompt, [`DEFAULT_MAX_ITERATIONS`] iterations.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        gateway: Arc<dyn LlmGateway>,
    ) -> Self {
        ReActAgent {
            name: name.into(),
            description: description.into(),
            gateway,
            tools: None,
            provider: LlmProvider::Anthropic,
            model: DEFAULT_MODEL.to_owned(),
            system: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            middleware: MiddlewareStack::new(),
            input_blocks_channel: None,
            event_sink: None,
        }
    }

    /// Bind the channel carrying this run's multimodal input blocks (ADR 0030 9e).
    pub fn with_input_blocks_channel(mut self, channel: impl Into<String>) -> Self {
        self.input_blocks_channel = Some(channel.into());
        self
    }

    /// Attach an observational token-delta sink (ADR 0033 phase 13). With a sink installed,
    /// each LLM call streams via `gateway.stream()` and emits a delta per token; without one,
    /// the loop calls `gateway.complete()` and behaves exactly as before. Opt-in: a run with
    /// no consumer keeps its current, byte-identical path.
    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Install the agent middleware stack (ADR 0025). Default is empty (the approval gate
    /// is still enforced — it is intrinsic to the stack). Efficiency middleware (compress,
    /// terse, context-budget trim) are installed here rather than as flat agent knobs.
    pub fn with_middleware(mut self, middleware: MiddlewareStack) -> Self {
        self.middleware = middleware;
        self
    }

    pub fn with_tools(mut self, tools: Arc<InMemoryToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Run the ReAct loop against the given input and channel snapshot.
    /// `approved_tool_names` are the `requires_approval` tools a human has already
    /// granted (e.g. injected into state on resume): they execute instead of being
    /// gated again.
    pub async fn run(
        &self,
        input: &Value,
        channels: &BTreeMap<String, Value>,
        approved_tool_names: &HashSet<String>,
        run_id: Option<&str>,
    ) -> Result<AgentResult, LlmError> {
        self.run_scoped(input, channels, approved_tool_names, None, run_id)
            .await
    }

    /// As [`Self::run`], but tags any emitted token deltas with `spawn_id` — the
    /// `mapAgents` sub-agent index (ADR 0033 phase 13b), so a consumer can demultiplex the
    /// interleaved streams of concurrent spawns. A top-level agent node passes `None`
    /// (via [`Self::run`]). Behaviourally identical to `run` apart from the delta tag; with
    /// no `event_sink` installed, `spawn_id` is unused.
    ///
    /// `run_id` (ADR 0043) is the run — or subgraph child run, `{parent_run_id}:{node_id}` —
    /// this loop is executing for. Tagged onto every `LlmRequest` it builds so a recorded
    /// journal can later discriminate a parent's calls from a recursively-replayed child's.
    /// `None` is honest for a caller with no run context (e.g. a unit test).
    pub async fn run_scoped(
        &self,
        input: &Value,
        channels: &BTreeMap<String, Value>,
        approved_tool_names: &HashSet<String>,
        spawn_id: Option<u32>,
        run_id: Option<&str>,
    ) -> Result<AgentResult, LlmError> {
        let mut trace: Vec<String> = Vec::new();
        let mut approval_requests: Vec<ApprovalRequestItem> = Vec::new();
        // The latest `writeTodos` result this run (ADR 0022/0023). The node handler
        // sinks it into the durable todos channel.
        let mut last_todos: Option<Vec<TodoItem>> = None;
        let mut memory_writes: Vec<MemoryWrite> = Vec::new();
        // ADR 0028 phase 7a: token usage summed across this run's LLM calls.
        let mut usage = LlmUsage::default();
        let tool_defs = self.build_tool_defs();

        // ADR 0030 9e: pull the run's multimodal input blocks from the bound channel (if any),
        // and exclude that channel from the stringified State so binary bytes are not re-fed as
        // text (and don't bloat the seed).
        let input_blocks: Option<Vec<ContentBlock>> = self
            .input_blocks_channel
            .as_deref()
            .and_then(|channel| channels.get(channel))
            .and_then(|value| serde_json::from_value::<Vec<ContentBlock>>(value.clone()).ok());

        // `Value`'s Display is compact JSON — same output as `serde_json::to_string`. The
        // full state is injected; a `ContextBudgetMiddleware` (ADR 0025 phase 3d, installed
        // when the SDK requests it) trims this seed message in `before_run` if a cap is set.
        let state_value = Value::Object(
            channels
                .iter()
                .filter(|(key, _)| Some(key.as_str()) != self.input_blocks_channel.as_deref())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        let state_str = state_value.to_string();
        let seed_text = format!("Input: {input}\nState: {state_str}");
        let mut conversation = match input_blocks {
            Some(blocks) if !blocks.is_empty() => {
                let mut all = vec![ContentBlock::Text { text: seed_text }];
                all.extend(blocks);
                vec![LlmMessage::with_blocks("user", all)]
            }
            _ => vec![LlmMessage::text("user", seed_text)],
        };

        // ADR 0025: `before_run` — fires once before the loop (empty stack = no-op). A
        // middleware may trim the seed state or stop the run.
        if let Flow::Stop { reason } = self
            .middleware
            .before_run(
                &mut conversation,
                &RunCtx {
                    iteration: 0,
                    approved_tool_names,
                    channels,
                    run_id,
                },
            )
            .await?
        {
            trace.push(format!("stopped:{reason}"));
            return Ok(AgentResult {
                reasoning: trace.join("\n"),
                requires_human_review: !approval_requests.is_empty(),
                approval_requests,
                todos: last_todos,
                usage: Some(usage),
                structured_output: None,
                memory_writes: if memory_writes.is_empty() {
                    None
                } else {
                    Some(memory_writes.clone())
                },
            });
        }

        'iterations: for iteration in 0..self.max_iterations {
            let ctx = RunCtx {
                iteration,
                approved_tool_names,
                channels,
                run_id,
            };
            // ADR 0025: `before_model` (fail-closed — an Err short-circuits the run).
            let request = self
                .middleware
                .before_model(
                    LlmRequest {
                        provider: self.provider,
                        model: self.model.clone(),
                        messages: conversation.clone(),
                        system: self.system.clone(),
                        tools: tool_defs.clone(),
                        max_tokens: None,
                        temperature: None,
                        response_format: None,
                        run_id: run_id.map(str::to_owned),
                    },
                    &ctx,
                )
                .await?;
            // ADR 0033 phase 13: opt-in token streaming. With a sink, drive
            // `gateway.stream()` and emit each delta (purely observational); the RETURNED
            // response is the authoritative, fully-assembled one — byte-identical to the
            // `complete()` path, so `after_model`, usage, history, and every checkpoint are
            // unchanged. Without a sink, the path is exactly as before.
            let response = match &self.event_sink {
                Some(sink) => {
                    // One stable id per agent turn: all deltas of this iteration share it,
                    // so a consumer concatenates them into a single streamed message. The
                    // sink (built in `bindings`) namespaces it with the run/spawn context.
                    let message_id = format!("turn-{iteration}");
                    let sink = Arc::clone(sink);
                    let emit = move |delta: &str| sink.token_delta(spawn_id, &message_id, delta);
                    self.gateway.stream(request.clone(), &emit).await?
                }
                None => self.gateway.complete(request.clone()).await?,
            };
            // ADR 0025: `after_model`.
            let response = self
                .middleware
                .after_model(response, &request, &ctx)
                .await?;

            // ADR 0028 phase 7a: accumulate token usage across the loop's LLM calls.
            usage.prompt_tokens += response.usage.prompt_tokens;
            usage.completion_tokens += response.usage.completion_tokens;
            if let Some(read) = response.usage.cache_read_tokens {
                usage.cache_read_tokens = Some(usage.cache_read_tokens.unwrap_or(0) + read);
            }
            if let Some(write) = response.usage.cache_write_tokens {
                usage.cache_write_tokens = Some(usage.cache_write_tokens.unwrap_or(0) + write);
            }

            let content = response.content.trim().to_owned();
            trace.push(format!("thought:{content}"));
            // ADR 0025: `on_iteration` (loop-detection / budget / reflection trigger).
            if let Flow::Stop { reason } = self.middleware.on_iteration(iteration, &content, &ctx) {
                trace.push(format!("stopped:{reason}"));
                break;
            }

            // Native tool-calling takes precedence over the text protocol.
            let tool_calls = response.tool_calls.unwrap_or_default();
            if !tool_calls.is_empty() {
                // Echo the assistant's tool-call turn into history WITH the structured
                // `tool_calls`, so the provider sees a coherent function-calling transcript
                // (assistant tool_call → tool result) and does not redundantly re-call.
                conversation.push(LlmMessage {
                    role: "assistant".to_owned(),
                    content: content.clone(),
                    tool_calls: Some(tool_calls.clone()),
                    tool_call_id: None,
                    tool_name: None,
                    content_blocks: None,
                });
                for call in &tool_calls {
                    let outcome = self
                        .execute_tool_call(
                            &call.name,
                            call.input.clone(),
                            Some(&call.id),
                            &ctx,
                            &mut trace,
                            &mut approval_requests,
                            &mut conversation,
                            &mut last_todos,
                            &mut memory_writes,
                        )
                        .await?;
                    if outcome == ToolOutcome::Approval {
                        break 'iterations;
                    }
                }
                continue;
            }

            // `ACTION: <tool> <json>` text-protocol call — execute, feed the
            // observation back, loop. Checked before the final-answer fallthrough.
            if let Some(rest) = content.strip_prefix("ACTION: ") {
                conversation.push(LlmMessage::text("assistant", content.clone()));
                let (tool_name, payload) = parse_action(rest);
                let outcome = self
                    .execute_tool_call(
                        &tool_name,
                        payload,
                        None,
                        &ctx,
                        &mut trace,
                        &mut approval_requests,
                        &mut conversation,
                        &mut last_todos,
                        &mut memory_writes,
                    )
                    .await?;
                if outcome == ToolOutcome::Approval {
                    break;
                }
                continue;
            }

            // Any tool-call-free, action-free text turn is the final answer.
            // Honour an explicit `FINAL:` marker wherever it appears (models
            // rarely put it first), otherwise take the whole text. We must NOT
            // loop here: re-querying would append an assistant-terminated
            // conversation, which strict OpenAI-compatible providers (e.g.
            // Mistral) reject with a 400.
            let answer = match content.find("FINAL:") {
                Some(index) => content[index + "FINAL:".len()..].trim(),
                None => content.as_str(),
            };
            trace.push(format!("final:{answer}"));
            break;
        }

        let mut result = AgentResult {
            reasoning: trace.join("\n"),
            requires_human_review: !approval_requests.is_empty(),
            approval_requests,
            todos: last_todos,
            usage: Some(usage),
            structured_output: None,
            memory_writes: if memory_writes.is_empty() {
                None
            } else {
                Some(memory_writes)
            },
        };
        // ADR 0025: `after_run` — finalize / reflection / metadata (empty stack = no-op).
        self.middleware
            .after_run(
                &mut result,
                &RunCtx {
                    iteration: self.max_iterations,
                    approved_tool_names,
                    channels,
                    run_id,
                },
            )
            .await?;
        Ok(result)
    }

    /// Resolve a tool by name and either execute it or gate it. Shared by the
    /// native tool-call path and the `ACTION:` text protocol so both honour the
    /// approval rule identically: a `requires_approval` tool is never self-executed.
    #[allow(clippy::too_many_arguments)]
    async fn execute_tool_call(
        &self,
        name: &str,
        input: Value,
        // `Some(id)` on the native function-calling path → the result is replayed as a
        // `role:"tool"` message linked to that call id. `None` on the `ACTION:` text
        // protocol → the result is a plain `user` observation (no structured id exists).
        tool_call_id: Option<&str>,
        // The loop-turn context (iteration / approved grants / channels) the middleware
        // hooks read. The approval gate reads `ctx.approved_tool_names`.
        ctx: &RunCtx<'_>,
        trace: &mut Vec<String>,
        approval_requests: &mut Vec<ApprovalRequestItem>,
        conversation: &mut Vec<LlmMessage>,
        // When the tool is `writeTodos`, its normalized list is captured here so the
        // node handler can persist it into the durable todos channel (ADR 0022/0023).
        last_todos: &mut Option<Vec<TodoItem>>,
        // `rememberMemory` write intents are accumulated here for the node handler to patch into
        // the durable `__memoryWrites` channel (ADR 0045 Stage 1b).
        memory_writes: &mut Vec<MemoryWrite>,
    ) -> Result<ToolOutcome, LlmError> {
        let resolved = self.tools.as_ref().and_then(|tools| tools.resolve(name));
        let Some((definition, handler)) = resolved else {
            trace.push(format!("observation:tool_not_found:{name}"));
            return Ok(ToolOutcome::NotFound);
        };

        // ADR 0025 phase 3c: the approval gate (now intrinsic to the stack) + any installed
        // before_tool middleware (fs policy, …) decide here. The gate is enforced even with
        // an empty stack — see `MiddlewareStack::before_tool`.
        let decision = {
            let call = ToolCallCtx {
                name: &definition.name,
                input: &input,
                requires_approval: definition.requires_approval,
                content_scoped: definition.content_scoped,
            };
            self.middleware.before_tool(&call, ctx).await?
        };
        let input = match decision {
            // Approval-gated and not granted: record the request and stop here — the agent
            // never self-approves.
            ToolControl::Gate(item) => {
                approval_requests.push(item);
                trace.push(format!("observation:approval_required:{name}"));
                return Ok(ToolOutcome::Approval);
            }
            // A middleware refused the call (e.g. fs policy): the handler never runs; feed
            // the denial back as an observation so the model can adapt or finalize.
            ToolControl::Deny { reason } => {
                let observation = format!("tool_error:denied:{reason}");
                trace.push(format!("observation:{observation}"));
                Self::push_observation(conversation, tool_call_id, name, observation);
                return Ok(ToolOutcome::Executed);
            }
            // Allowed, possibly with an overridden input.
            ToolControl::Allow { input_override } => input_override.unwrap_or(input),
        };

        // A handler error is data, not a crash: it goes back to the model as an
        // observation so the loop can recover or finalize.
        let input_for_after = input.clone();
        let raw = match handler(input).await {
            Ok(value) => {
                // `writeTodos` returns the authoritative normalized list — capture it
                // (last write wins) for the node handler to persist durably.
                if name == WRITE_TODOS_TOOL {
                    if let Ok(todos) = serde_json::from_value::<Vec<TodoItem>>(value.clone()) {
                        *last_todos = Some(todos);
                    }
                }
                // `rememberMemory` returns the durable key — capture the write intent (key + the
                // remembered text) for the node handler to drain into `__memoryWrites`.
                if name == REMEMBER_MEMORY_TOOL {
                    if let (Some(key), Some(text)) = (
                        value.get("key").and_then(Value::as_str),
                        input_for_after.get("text").and_then(Value::as_str),
                    ) {
                        memory_writes.push(MemoryWrite {
                            op: "remember".to_owned(),
                            key: key.to_owned(),
                            text: text.to_owned(),
                        });
                    }
                }
                value
            }
            Err(message) => Value::String(format!("tool_error:{message}")),
        };
        // ADR 0025 phase 3c: `after_tool` may transform the observation (empty stack = no-op).
        let output = self
            .middleware
            .after_tool(name, &input_for_after, raw, ctx)
            .await?
            .to_string();
        trace.push(format!("observation:{output}"));
        Self::push_observation(conversation, tool_call_id, name, output);
        Ok(ToolOutcome::Executed)
    }

    /// Record a tool observation into the conversation: a structured `role:"tool"` message
    /// linked to the call id on the native path (the provider recognises the tool was
    /// answered and finalises instead of re-calling), or a plain `user` observation on the
    /// `ACTION:` text protocol (no structured id exists).
    fn push_observation(
        conversation: &mut Vec<LlmMessage>,
        tool_call_id: Option<&str>,
        name: &str,
        output: String,
    ) {
        match tool_call_id {
            Some(id) => conversation.push(LlmMessage {
                role: "tool".to_owned(),
                content: output,
                tool_calls: None,
                tool_call_id: Some(id.to_owned()),
                tool_name: Some(name.to_owned()),
                content_blocks: None,
            }),
            None => conversation.push(LlmMessage::text("user", format!("observation:{output}"))),
        }
    }

    /// Advertise tools to the LLM: only those carrying a JSON Schema, with their
    /// description — the shape the provider needs to emit native tool calls.
    fn build_tool_defs(&self) -> Option<Vec<LlmToolDef>> {
        let tools = self.tools.as_ref()?;
        let defs: Vec<LlmToolDef> = tools
            .list()
            .into_iter()
            .filter_map(|definition| {
                definition.input_schema.clone().map(|schema| LlmToolDef {
                    name: definition.name.clone(),
                    description: Some(definition.description.clone()),
                    input_schema: schema,
                })
            })
            .collect();
        if defs.is_empty() {
            None
        } else {
            Some(defs)
        }
    }
}

/// Parse the remainder of an `ACTION: <tool> <optional json>` line. A missing or
/// unparsable payload defaults to `{}` (the TS protocol's default input).
fn parse_action(rest: &str) -> (String, Value) {
    let rest = rest.trim_start();
    let (name, payload_text) = match rest.split_once(char::is_whitespace) {
        Some((name, payload)) => (name.to_owned(), payload.trim().to_owned()),
        None => (rest.to_owned(), String::new()),
    };
    let payload = if payload_text.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(&payload_text)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    };
    (name, payload)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use ailu_llm_gateway::{DefaultLlmGateway, LlmResponse, LlmToolCall, LlmUsage, MockAdapter};
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::tools::{sync_tool, ToolDefinition};

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

    fn tool_use(name: &str) -> LlmResponse {
        LlmResponse {
            content: String::new(),
            tool_calls: Some(vec![LlmToolCall {
                id: "tu1".to_owned(),
                name: name.to_owned(),
                input: json!({}),
            }]),
            stop_reason: Some("tool_use".to_owned()),
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

    /// A gateway that records the `run_id` of every request it sees (ADR 0043) — used to
    /// prove the REAL value passed to `run`/`run_scoped` actually reaches the `LlmRequest`,
    /// not just that the parameter compiles.
    #[derive(Default)]
    struct RunIdCapturingGateway {
        seen: Mutex<Vec<Option<String>>>,
    }

    #[async_trait]
    impl LlmGateway for RunIdCapturingGateway {
        async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
            self.seen.lock().expect("lock").push(request.run_id);
            Ok(text("FINAL: done"))
        }
    }

    #[tokio::test]
    async fn run_tags_every_llm_request_with_the_given_run_id() {
        let gateway = Arc::new(RunIdCapturingGateway::default());
        let agent = ReActAgent::new("a", "t", gateway.clone());
        agent
            .run(
                &json!({}),
                &BTreeMap::new(),
                &HashSet::new(),
                Some("run-42"),
            )
            .await
            .unwrap();
        assert_eq!(
            gateway.seen.lock().unwrap().as_slice(),
            &[Some("run-42".to_owned())]
        );
    }

    #[tokio::test]
    async fn run_with_no_run_id_tags_the_request_none() {
        let gateway = Arc::new(RunIdCapturingGateway::default());
        let agent = ReActAgent::new("a", "t", gateway.clone());
        agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert_eq!(gateway.seen.lock().unwrap().as_slice(), &[None]);
    }

    /// A test [`EventSink`] that records every observed token delta with its tags.
    #[derive(Default)]
    struct RecordingSink {
        deltas: Mutex<Vec<(Option<u32>, String, String)>>,
    }
    impl EventSink for RecordingSink {
        fn token_delta(&self, spawn_id: Option<u32>, message_id: &str, delta: &str) {
            self.deltas.lock().expect("lock").push((
                spawn_id,
                message_id.to_owned(),
                delta.to_owned(),
            ));
        }
    }

    /// A gateway whose adapter streams the scripted deltas, returning `content` assembled.
    fn streaming_gateway_with(
        responses: Vec<LlmResponse>,
        scripts: Vec<Vec<String>>,
    ) -> Arc<DefaultLlmGateway> {
        let mut gateway = DefaultLlmGateway::new();
        gateway.register_adapter(Box::new(
            MockAdapter::new(LlmProvider::Anthropic, responses).with_stream_scripts(scripts),
        ));
        Arc::new(gateway)
    }

    /// ADR 0033 phase 13 — the determinism guard: the streaming path is byte-identical to
    /// the `complete()` path (same `AgentResult`), AND the deltas are observed in order,
    /// tagged as a top-level turn (`spawn_id` None, `message_id` "turn-0").
    #[tokio::test]
    async fn token_streaming_is_byte_identical_and_emits_ordered_deltas() {
        let script = vec![vec!["FINAL".to_owned(), ": do".to_owned(), "ne".to_owned()]];

        // Baseline: no sink → complete().
        let baseline = ReActAgent::new(
            "a",
            "t",
            streaming_gateway_with(vec![text("FINAL: done")], script.clone()),
        )
        .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
        .await
        .unwrap();

        // Streaming: a sink → stream().
        let sink = Arc::new(RecordingSink::default());
        let streamed = ReActAgent::new(
            "a",
            "t",
            streaming_gateway_with(vec![text("FINAL: done")], script),
        )
        .with_event_sink(sink.clone())
        .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
        .await
        .unwrap();

        // The result is identical — streaming changes nothing the run records.
        assert_eq!(baseline, streamed);

        let deltas = sink.deltas.lock().expect("lock");
        let texts: Vec<&str> = deltas.iter().map(|(_, _, d)| d.as_str()).collect();
        assert_eq!(texts, vec!["FINAL", ": do", "ne"]);
        assert!(deltas.iter().all(|(spawn, _, _)| spawn.is_none()));
        assert_eq!(deltas[0].1, "turn-0");
    }

    /// ADR 0033 phase 13b: `run_scoped` tags every emitted delta with its `spawn_id` (the
    /// `mapAgents` sub-agent index), so concurrent spawns' interleaved streams demultiplex.
    #[tokio::test]
    async fn run_scoped_tags_deltas_with_the_spawn_id() {
        let sink = Arc::new(RecordingSink::default());
        let agent = ReActAgent::new(
            "w",
            "sub",
            streaming_gateway_with(
                vec![text("FINAL: ok")],
                vec![vec!["FINAL: ".to_owned(), "ok".to_owned()]],
            ),
        )
        .with_event_sink(sink.clone());

        agent
            .run_scoped(&json!({}), &BTreeMap::new(), &HashSet::new(), Some(3), None)
            .await
            .unwrap();

        let deltas = sink.deltas.lock().expect("lock");
        assert!(!deltas.is_empty());
        assert!(deltas.iter().all(|(spawn, _, _)| *spawn == Some(3)));
    }

    /// A gateway that captures the first request, to inspect the seed message (ADR 0030 9e).
    struct CapturingGateway {
        last: Mutex<Option<LlmRequest>>,
    }

    #[async_trait]
    impl LlmGateway for CapturingGateway {
        async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
            let mut slot = self.last.lock().unwrap();
            if slot.is_none() {
                *slot = Some(request.clone());
            }
            Ok(text("FINAL: ok"))
        }
    }

    #[tokio::test]
    async fn input_blocks_channel_builds_a_multimodal_seed_and_excludes_the_channel() {
        let gateway = Arc::new(CapturingGateway {
            last: Mutex::new(None),
        });
        let agent = ReActAgent::new("a", "", gateway.clone()).with_input_blocks_channel("__media");

        let mut channels = BTreeMap::new();
        channels.insert(
            "__media".to_owned(),
            json!([{ "type": "image", "source": { "kind": "base64", "mediaType": "image/png", "data": "AAAA" } }]),
        );
        channels.insert("topic".to_owned(), json!("cats"));
        let approved = HashSet::new();

        agent
            .run(&Value::Null, &channels, &approved, None)
            .await
            .unwrap();

        let req = gateway
            .last
            .lock()
            .unwrap()
            .clone()
            .expect("a request was sent");
        let seed = &req.messages[0];
        let blocks = seed.content_blocks.as_ref().expect("a multimodal seed");
        // First block = the text digest: it carries the rest of State but NOT the media channel
        // (no re-feeding binary bytes as text).
        match &blocks[0] {
            ContentBlock::Text { text } => {
                assert!(text.contains("topic"), "state still injected");
                assert!(
                    !text.contains("__media"),
                    "media channel excluded from State"
                );
                assert!(
                    !text.contains("AAAA"),
                    "image bytes not stringified into the seed"
                );
            }
            other => panic!("expected a leading text block, got {other:?}"),
        }
        // Second block = the image itself.
        assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
    }

    #[tokio::test]
    async fn no_input_blocks_channel_keeps_a_text_only_seed() {
        let gateway = Arc::new(CapturingGateway {
            last: Mutex::new(None),
        });
        let agent = ReActAgent::new("a", "", gateway.clone());
        let mut channels = BTreeMap::new();
        channels.insert("topic".to_owned(), json!("cats"));
        let approved = HashSet::new();

        agent
            .run(&Value::Null, &channels, &approved, None)
            .await
            .unwrap();

        let req = gateway
            .last
            .lock()
            .unwrap()
            .clone()
            .expect("a request was sent");
        assert!(
            req.messages[0].content_blocks.is_none(),
            "text-only seed unchanged"
        );
        assert!(req.messages[0].content.contains("topic"));
    }

    fn counting_tool(
        name: &str,
        requires_approval: bool,
        calls: &Arc<AtomicUsize>,
    ) -> (ToolDefinition, crate::tools::ToolHandler) {
        let counter = Arc::clone(calls);
        (
            ToolDefinition {
                name: name.to_owned(),
                description: format!("The {name} tool."),
                requires_approval,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
            },
            sync_tool(move |_input| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }),
        )
    }

    #[tokio::test]
    async fn final_stops_the_loop() {
        let agent = ReActAgent::new("a", "test agent", gateway_with(vec![text("FINAL: done")]));
        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert!(result.reasoning.contains("final:done"));
        assert!(!result.requires_human_review);
        assert!(result.approval_requests.is_empty());
    }

    /// ADR 0028 phase 7a: `AgentResult.usage` sums token usage across the loop's LLM calls.
    #[tokio::test]
    async fn usage_is_summed_across_the_loop() {
        let usage = |p: u32, c: u32| LlmUsage {
            prompt_tokens: p,
            completion_tokens: c,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        // Turn 1: a tool call (usage 10/5). Turn 2: the final answer (usage 8/3) → summed 18/8.
        let tool_turn = LlmResponse {
            usage: usage(10, 5),
            ..tool_use("noop")
        };
        let final_turn = LlmResponse {
            usage: usage(8, 3),
            ..text("FINAL: done")
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = InMemoryToolRegistry::new();
        let noop = counting_tool("noop", false, &calls);
        registry.register(noop.0, noop.1);

        let agent = ReActAgent::new("a", "test agent", gateway_with(vec![tool_turn, final_turn]))
            .with_tools(Arc::new(registry));
        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();

        let summed = result.usage.expect("usage present");
        assert_eq!(summed.prompt_tokens, 18);
        assert_eq!(summed.completion_tokens, 8);
    }

    #[tokio::test]
    async fn native_tool_use_executes_then_finalizes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = InMemoryToolRegistry::new();
        let weather = counting_tool("weather", false, &calls);
        registry.register(weather.0, weather.1);

        let agent = ReActAgent::new(
            "a",
            "weather agent",
            gateway_with(vec![tool_use("weather"), text("FINAL: 21C")]),
        )
        .with_tools(Arc::new(registry));

        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(result.reasoning.contains("observation:{\"ok\":true}"));
        assert!(result.reasoning.contains("final:21C"));
        assert!(!result.requires_human_review);
    }

    #[tokio::test]
    async fn requires_approval_gates_the_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = InMemoryToolRegistry::new();
        let deploy = counting_tool("deploy", true, &calls);
        registry.register(deploy.0, deploy.1);

        let agent = ReActAgent::new(
            "a",
            "deploy agent",
            gateway_with(vec![tool_use("deploy"), text("FINAL: deployed")]),
        )
        .with_tools(Arc::new(registry));

        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(result.requires_human_review);
        assert_eq!(result.approval_requests.len(), 1);
        assert_eq!(result.approval_requests[0].subject, "tool:deploy");
        assert!(result
            .reasoning
            .contains("observation:approval_required:deploy"));
    }

    #[tokio::test]
    async fn approved_set_unlocks_the_gated_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = InMemoryToolRegistry::new();
        let deploy = counting_tool("deploy", true, &calls);
        registry.register(deploy.0, deploy.1);

        let agent = ReActAgent::new(
            "a",
            "deploy agent",
            gateway_with(vec![tool_use("deploy"), text("FINAL: deployed")]),
        )
        .with_tools(Arc::new(registry));

        let approved: HashSet<String> = ["deploy".to_owned()].into_iter().collect();
        let result = agent
            .run(&json!({}), &BTreeMap::new(), &approved, None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!result.requires_human_review);
        assert!(result.approval_requests.is_empty());
        assert!(result.reasoning.contains("final:deployed"));
    }

    #[tokio::test]
    async fn action_text_protocol_executes_with_parsed_input() {
        let seen: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let capture = Arc::clone(&seen);
        let mut registry = InMemoryToolRegistry::new();
        registry.register(
            ToolDefinition {
                name: "echo".to_owned(),
                description: "Echoes its input.".to_owned(),
                requires_approval: false,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
            },
            sync_tool(move |input| {
                *capture.lock().expect("lock") = Some(input.clone());
                Ok(input)
            }),
        );

        let agent = ReActAgent::new(
            "a",
            "echo agent",
            gateway_with(vec![text("ACTION: echo {\"x\":1}"), text("FINAL: ok")]),
        )
        .with_tools(Arc::new(registry));

        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert_eq!(*seen.lock().expect("lock"), Some(json!({ "x": 1 })));
        assert!(result.reasoning.contains("observation:{\"x\":1}"));
        assert!(result.reasoning.contains("final:ok"));
    }

    #[tokio::test]
    async fn unknown_tool_is_traced_and_the_loop_continues() {
        let agent = ReActAgent::new(
            "a",
            "toolless agent",
            gateway_with(vec![tool_use("missing"), text("FINAL: ok")]),
        );
        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert!(result
            .reasoning
            .contains("observation:tool_not_found:missing"));
        assert!(result.reasoning.contains("final:ok"));
        assert!(!result.requires_human_review);
    }

    /// A gateway that counts `complete()` calls, to prove the loop does not
    /// re-query after an assistant-terminated text turn (the Mistral 400 bug).
    struct CountingGateway {
        responses: Vec<LlmResponse>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmGateway for CountingGateway {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
            let next = self.calls.fetch_add(1, Ordering::SeqCst);
            let index = next.min(self.responses.len() - 1);
            Ok(self.responses[index].clone())
        }
    }

    #[tokio::test]
    async fn final_marker_anywhere_in_the_text_terminates_with_the_suffix() {
        let agent = ReActAgent::new(
            "a",
            "test agent",
            gateway_with(vec![text("Here is the answer.\nFINAL: 42")]),
        );
        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        // The suffix after `FINAL:` is the answer; the prefix is dropped.
        assert!(result.reasoning.contains("final:42"));
        assert!(!result.reasoning.contains("final:Here is the answer."));
        assert!(!result.requires_human_review);
    }

    #[tokio::test]
    async fn plain_text_without_final_terminates_as_final_with_a_single_llm_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gateway = Arc::new(CountingGateway {
            // Two responses scripted, but the loop must stop after the first:
            // a plain text turn is the final answer, never re-queried.
            responses: vec![
                text("The answer is plainly stated."),
                text("FINAL: should-not-run"),
            ],
            calls: Arc::clone(&calls),
        });
        let agent = ReActAgent::new("a", "test agent", gateway);

        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();

        // Exactly one completion — no assistant-terminated re-query.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The whole text becomes the final answer when no `FINAL:` is present.
        assert!(result
            .reasoning
            .contains("final:The answer is plainly stated."));
        assert!(!result.requires_human_review);
    }

    #[test]
    fn agent_result_serializes_camel_case() {
        let result = AgentResult {
            reasoning: "thought:x".to_owned(),
            approval_requests: vec![ApprovalRequestItem {
                subject: "tool:deploy".to_owned(),
                reason: "Tool 'deploy' requires human approval before execution.".to_owned(),
                approval_key: None,
                input: None,
            }],
            requires_human_review: true,
            todos: None,
            usage: None,
            structured_output: None,
            memory_writes: None,
        };
        let wire = serde_json::to_string(&result).expect("serializes");
        assert!(wire.contains("\"approvalRequests\""));
        assert!(wire.contains("\"requiresHumanReview\":true"));
        // `todos` is omitted from the wire when absent — keeps old payloads valid.
        assert!(!wire.contains("\"todos\""));
        let back: AgentResult = serde_json::from_str(&wire).expect("round-trips");
        assert_eq!(back, result);
    }

    #[tokio::test]
    async fn write_todos_tool_call_populates_result_todos() {
        use crate::todos::{write_todos_tool, TodoStatus};

        let mut registry = InMemoryToolRegistry::new();
        let (definition, handler) = write_todos_tool();
        registry.register(definition, handler);

        // Native tool call carrying a todo payload, then a final answer.
        let tool_call = LlmResponse {
            content: String::new(),
            tool_calls: Some(vec![LlmToolCall {
                id: "tu1".to_owned(),
                name: "writeTodos".to_owned(),
                input: json!({
                    "todos": [
                        { "text": "plan", "status": "in_progress" },
                        { "text": "build", "status": "pending" }
                    ]
                }),
            }]),
            stop_reason: Some("tool_use".to_owned()),
            usage: LlmUsage::default(),
            model: "mock".to_owned(),
            provider: LlmProvider::Anthropic,
            content_blocks: None,
        };

        let agent = ReActAgent::new(
            "a",
            "planner",
            gateway_with(vec![tool_call, text("FINAL: done")]),
        )
        .with_tools(Arc::new(registry));

        let result = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();

        let todos = result.todos.expect("todos captured");
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].id, "todo-1");
        assert_eq!(todos[0].status, TodoStatus::InProgress);
        assert_eq!(todos[1].id, "todo-2");
        assert!(result.reasoning.contains("final:done"));
        assert!(!result.requires_human_review);
    }

    /// ADR 0024 phase 2c — the over-grant guard: a content-scoped tool's approval is
    /// pinned to the exact input. Approving one call must NOT unlock a different input.
    #[tokio::test]
    async fn content_scoped_gate_pins_approval_to_the_exact_input() {
        use crate::tools::approval_key;

        fn guarded_call(input: Value) -> LlmResponse {
            LlmResponse {
                content: String::new(),
                tool_calls: Some(vec![LlmToolCall {
                    id: "t1".to_owned(),
                    name: "guarded".to_owned(),
                    input,
                }]),
                stop_reason: Some("tool_use".to_owned()),
                usage: LlmUsage::default(),
                model: "mock".to_owned(),
                provider: LlmProvider::Anthropic,
                content_blocks: None,
            }
        }
        fn guarded_agent(calls: &Arc<AtomicUsize>, responses: Vec<LlmResponse>) -> ReActAgent {
            let counter = Arc::clone(calls);
            let mut registry = InMemoryToolRegistry::new();
            registry.register(
                ToolDefinition {
                    name: "guarded".to_owned(),
                    description: "content-scoped guarded tool".to_owned(),
                    requires_approval: true,
                    input_schema: Some(json!({ "type": "object" })),
                    content_scoped: true,
                },
                sync_tool(move |_input| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({ "ok": true }))
                }),
            );
            ReActAgent::new("a", "g", gateway_with(responses)).with_tools(Arc::new(registry))
        }

        let input_a = json!({ "path": "review/a.md", "content": "X" });
        let input_b = json!({ "path": "review/b.md", "content": "Y" });
        let key_a = approval_key("guarded", true, &input_a);
        let key_b = approval_key("guarded", true, &input_b);
        assert!(key_a.starts_with("guarded#") && key_a != key_b);

        // 1) Unapproved → gates, recording the content-scoped key + input for THIS call.
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = guarded_agent(
            &calls,
            vec![guarded_call(input_a.clone()), text("FINAL: x")],
        );
        let r = agent
            .run(&json!({}), &BTreeMap::new(), &HashSet::new(), None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(r.requires_human_review);
        assert_eq!(
            r.approval_requests[0].approval_key.as_deref(),
            Some(key_a.as_str())
        );
        assert_eq!(r.approval_requests[0].input.as_ref(), Some(&input_a));

        // 2) Granted A's key → the SAME call executes.
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = guarded_agent(
            &calls,
            vec![guarded_call(input_a.clone()), text("FINAL: x")],
        );
        let approved: HashSet<String> = [key_a.clone()].into_iter().collect();
        let r = agent
            .run(&json!({}), &BTreeMap::new(), &approved, None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!r.requires_human_review);

        // 3) Granted A, but the agent calls a DIFFERENT input B → re-gates (no over-grant).
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = guarded_agent(
            &calls,
            vec![guarded_call(input_b.clone()), text("FINAL: x")],
        );
        let approved: HashSet<String> = [key_a].into_iter().collect();
        let r = agent
            .run(&json!({}), &BTreeMap::new(), &approved, None)
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a different input must NOT reuse the grant"
        );
        assert!(r.requires_human_review);
        assert_eq!(
            r.approval_requests[0].approval_key.as_deref(),
            Some(key_b.as_str())
        );
    }
}
