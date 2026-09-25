//! Shared pure-Rust SDK core: JSON-string in / JSON-string out, with no host
//! language types. Every public function returns `Result<String, String>` so each
//! binding layer can map errors into its native exception/result type.
//!
//! This is where callback-neutral SDK work happens: graph validation, DSL
//! compilation, the model policy, the component/prebuilt catalogs, and the two
//! fully-Rust run paths (`run_component`, `run_prebuilt`). The gateway/run logic
//! mirrors the napi bridge (`crates/bindings/src/bridge.rs`) minus host-language
//! callback seams, so a run drives a current-thread tokio runtime with `block_on`
//! and cannot deadlock.

use std::collections::BTreeMap;
use std::sync::Arc;

use ailu_agents_core::{agent_node_handler, InMemoryToolRegistry, ReActAgent, ToolDefinition};
use ailu_components::{
    list_prebuilt as list_prebuilt_rs, prebuilt, ComponentRegistry, PrebuiltAgent,
};
use ailu_graph_ailu::compile_graph_yaml as compile_graph_yaml_rs;
use ailu_graph_core::{
    validate_graph, ChannelDefinition, ChannelReducer, GraphDefinition, GraphId, GraphState,
    GraphStatus, NodeDefinition, NodeId, NodeType, RunId,
};
use ailu_graph_runtime::{
    GraphRuntime, InMemoryConditionRegistry, InMemoryNodeRegistry, NodeRegistry,
};
use ailu_llm_gateway::{
    missing_credentials_message, offline_mock_enabled, provider_key_from_env, AnthropicAdapter,
    DefaultLlmGateway, GeminiAdapter, HttpAnthropicPort, HttpGeminiPort, LlmProvider, LlmResponse,
    LlmUsage, MockAdapter, ModelChoice, ModelPolicy, ModelTier, OpenAiCompatibleAdapter,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Graph validation / DSL compilation (mirrors the original surface)
// ---------------------------------------------------------------------------

/// Version of the bound Rust engine.
#[must_use]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Validate a graph definition (JSON string). Returns a JSON array of validation
/// errors — empty (`[]`) when the graph is structurally sound. `Err` on malformed
/// JSON.
pub fn validate_graph_json(definition_json: &str) -> Result<String, String> {
    let definition: GraphDefinition = serde_json::from_str(definition_json)
        .map_err(|error| format!("invalid graph JSON: {error}"))?;
    let errors = validate_graph(&definition);
    serde_json::to_string(&errors).map_err(|error| error.to_string())
}

/// Compile graph DSL YAML into a validated `GraphDefinition` (JSON string). `Err` on
/// parse, DSL, or structural validation failure.
pub fn compile_graph_yaml(yaml: &str) -> Result<String, String> {
    let definition = compile_graph_yaml_rs(yaml).map_err(|error| error.to_string())?;
    serde_json::to_string(&definition).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Model policy
// ---------------------------------------------------------------------------

/// Resolve a capability tier to a concrete `{ provider, model, recommended }`
/// (the [`ModelChoice`] JSON).
///
/// - `tier` is a `ModelTier` string (`"frontier" | "balanced" | "fast" | "creative"`).
/// - `available_json`, when given, is a JSON array of provider strings
///   (e.g. `["mistral"]`); when absent the providers are derived from the process
///   env via [`ModelPolicy::available_from_env`].
/// - `override_json`, when given, is `{ "provider"?: string, "model"?: string }`;
///   either field overrides the policy choice (and flags `recommended = false`).
pub fn resolve_model(
    tier: &str,
    available_json: Option<&str>,
    override_json: Option<&str>,
) -> Result<String, String> {
    let tier = parse_tier(tier)?;
    let policy = ModelPolicy::default();

    let available: Vec<LlmProvider> = match available_json {
        Some(raw) => parse_provider_array(raw)?,
        None => policy.available_from_env(),
    };

    let (override_provider, override_model) = match override_json {
        Some(raw) => parse_override(raw)?,
        None => (None, None),
    };

    let choice = policy.resolve(
        tier,
        &available,
        override_provider,
        override_model.as_deref(),
    );
    serde_json::to_string(&choice).map_err(|error| error.to_string())
}

/// The providers usable in the current process env (a JSON array of provider
/// strings), as decided by [`ModelPolicy::available_from_env`].
pub fn available_providers() -> Result<String, String> {
    let providers = ModelPolicy::default().available_from_env();
    serde_json::to_string(&providers).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Catalogs
// ---------------------------------------------------------------------------

/// The component kinds the [`ComponentRegistry`] knows how to build (JSON array of
/// strings).
pub fn list_components() -> Result<String, String> {
    serde_json::to_string(ComponentRegistry::kinds()).map_err(|error| error.to_string())
}

/// Every prebuilt micro-agent definition (JSON array of [`PrebuiltAgent`],
/// camelCase).
pub fn list_prebuilt() -> Result<String, String> {
    let agents: Vec<PrebuiltAgent> = list_prebuilt_rs();
    serde_json::to_string(&agents).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Run paths (fully on Rust)
// ---------------------------------------------------------------------------

/// Run a single component handler, fully on Rust.
///
/// Builds the handler for `kind` + `params_json` via [`ComponentRegistry`], seeds a
/// [`GraphState`] whose `channels` are the parsed `channels_json` object, drives the
/// handler on a current-thread tokio runtime, and returns the resulting channel
/// update map (the component's output patch) as a JSON object.
///
/// `Err` if the kind/params are invalid, the channels JSON is not an object, or the
/// handler reports a failure.
pub fn run_component(kind: &str, params_json: &str, channels_json: &str) -> Result<String, String> {
    let params: Value = serde_json::from_str(params_json)
        .map_err(|error| format!("invalid params JSON: {error}"))?;
    let channels = parse_channels(channels_json)?;

    let handler = ComponentRegistry::new()
        .build_handler(kind, &params)
        .map_err(|error| format!("component '{kind}': {error}"))?;

    let state = make_state(channels);
    let output = block_on(handler(state));

    if let Some(reason) = output.failure {
        return Err(format!("component '{kind}' failed: {reason}"));
    }

    let update: BTreeMap<String, Value> = output.update.into_iter().collect();
    serde_json::to_string(&update).map_err(|error| error.to_string())
}

/// Run a prebuilt micro-agent, fully on Rust.
///
/// Looks up the [`PrebuiltAgent`] by `name`, resolves its model via
/// [`ModelPolicy`] (its `tier` + the env-available providers, honouring an optional
/// `{ provider?, model? }` override in `options_json`), builds a Rust gateway from
/// env (mistral when `MISTRAL_API_KEY`, anthropic when `ANTHROPIC_API_KEY`, ollama
/// when `AILU_USE_OLLAMA=1`; with no credentials it fails loud unless offline mode
/// (`AILU_LLM_MOCK=1`) runs the deterministic mock — mirroring the napi bridge's `build_gateway`), assembles a one-agent graph writing to the agent's
/// `output_channel`, runs it via [`GraphRuntime`], and returns a `RunOutcome` JSON:
/// `{ status, channels, resolvedModel: { provider, model } }`.
///
/// `input_json` is the agent input, seeded into an `input` channel of the run's
/// initial state (the agent reads the channel snapshot). `Err` on an unknown agent
/// name, malformed JSON, or a runtime error.
pub fn run_prebuilt(
    name: &str,
    input_json: &str,
    options_json: Option<&str>,
) -> Result<String, String> {
    let agent_def = prebuilt(name).ok_or_else(|| format!("unknown prebuilt agent '{name}'"))?;

    let input: Value =
        serde_json::from_str(input_json).map_err(|error| format!("invalid input JSON: {error}"))?;

    let (override_provider, override_model) = match options_json {
        Some(raw) => parse_override(raw)?,
        None => (None, None),
    };

    // Resolve the concrete model the agent runs with BEFORE building the gateway,
    // so the registered adapter and the agent's provider/model agree.
    let policy = ModelPolicy::default();
    let available = policy.available_from_env();
    let resolved = policy.resolve(
        agent_def.tier,
        &available,
        override_provider,
        override_model.as_deref(),
    );

    let gateway = build_gateway(&resolved)?;

    let mut registry = InMemoryToolRegistry::new();
    for tool_name in &agent_def.tool_names {
        // `writeTodos` has a real Rust impl (ADR 0022/0023) — register it verbatim,
        // never the no-op stub, so a Python agent gets the real planning tool too.
        if tool_name == ailu_agents_core::WRITE_TODOS_TOOL {
            let (definition, handler) = ailu_agents_core::write_todos_tool();
            registry.register(definition, handler);
            continue;
        }
        let requires_approval = agent_def.suspend_for_approval;
        registry.register(
            ToolDefinition {
                name: tool_name.clone(),
                description: format!("Tool '{tool_name}'."),
                requires_approval,
                input_schema: Some(json!({ "type": "object" })),
                content_scoped: false,
            },
            // Deterministic no-op tool so the agent loop can observe a result.
            ailu_agents_core::sync_tool({
                let name = tool_name.clone();
                move |_input| Ok(json!({ "tool": name, "ok": true }))
            }),
        );
    }

    let agent = ReActAgent::new(
        agent_def.name.clone(),
        agent_def.description.clone(),
        gateway,
    )
    .with_provider(resolved.provider)
    .with_model(resolved.model.clone())
    .with_system(agent_def.system_prompt.clone())
    .with_tools(Arc::new(registry));

    let output_channel = agent_def.output_channel.clone();
    let handler = agent_node_handler(
        Arc::new(agent),
        output_channel.clone(),
        agent_def.suspend_for_approval,
        // The Python prebuilt agent has no durable todos channel yet (TS-SDK parity is
        // phase 1; a Python todosChannel surface is future parity work) — no sink.
        None,
    );

    let node_id = "agent";
    let mut nodes = InMemoryNodeRegistry::new();
    nodes.register(NodeId::from(node_id), handler);

    let graph = GraphDefinition {
        id: GraphId::from("prebuilt"),
        version: "0.0.0".to_owned(),
        name: name.to_owned(),
        recursion_limit: None,
        channels: [
            ("input".to_owned(), replace_channel()),
            (output_channel.clone(), replace_channel()),
        ]
        .into_iter()
        .collect(),
        nodes: vec![agent_node(node_id)],
        edges: vec![],
        entry_node_id: NodeId::from(node_id),
        metadata: None,
    };

    let runtime = GraphRuntime::new(graph, nodes, InMemoryConditionRegistry::new());

    let initial: BTreeMap<String, Value> = [("input".to_owned(), input)].into_iter().collect();
    let final_state = block_on(runtime.start(RunId::from("run-prebuilt"), initial))
        .map_err(|error| format!("runtime error: {error}"))?;

    let status = serde_json::to_value(final_state.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let channels: BTreeMap<String, Value> = final_state.channels.into_iter().collect();

    let outcome = json!({
        "status": status,
        "channels": channels,
        "resolvedModel": {
            "provider": resolved.provider,
            "model": resolved.model,
        },
    });
    serde_json::to_string(&outcome).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a `ModelTier` from its camelCase wire string.
fn parse_tier(tier: &str) -> Result<ModelTier, String> {
    serde_json::from_value::<ModelTier>(Value::String(tier.to_owned()))
        .map_err(|_| format!("unknown model tier '{tier}'"))
}

/// Parse a JSON array of provider strings (e.g. `["mistral","ollama"]`).
fn parse_provider_array(raw: &str) -> Result<Vec<LlmProvider>, String> {
    serde_json::from_str::<Vec<LlmProvider>>(raw)
        .map_err(|error| format!("invalid available providers JSON: {error}"))
}

/// Parse an optional `{ provider?, model? }` override object. An absent/`null`
/// document yields `(None, None)`.
fn parse_override(raw: &str) -> Result<(Option<LlmProvider>, Option<String>), String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid override JSON: {error}"))?;
    if value.is_null() {
        return Ok((None, None));
    }
    let object = value
        .as_object()
        .ok_or_else(|| "override must be a JSON object".to_owned())?;

    let provider = match object.get("provider") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(
            serde_json::from_value::<LlmProvider>(Value::String(s.clone()))
                .map_err(|_| format!("unknown provider '{s}'"))?,
        ),
        Some(_) => return Err("override.provider must be a string".to_owned()),
    };
    let model = match object.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("override.model must be a string".to_owned()),
    };
    Ok((provider, model))
}

/// Parse a JSON object of channels into the runtime channel map.
fn parse_channels(raw: &str) -> Result<BTreeMap<String, Value>, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid channels JSON: {error}"))?;
    match value {
        Value::Object(map) => Ok(map.into_iter().collect()),
        _ => Err("channels must be a JSON object".to_owned()),
    }
}

/// A minimal `GraphState` seeded with the given channels — enough for a component
/// handler, which only reads `state.channels`.
fn make_state(channels: BTreeMap<String, Value>) -> GraphState {
    GraphState {
        run_id: RunId::from("run-component"),
        graph_id: GraphId::from("component"),
        current_node_id: NodeId::from("component"),
        status: GraphStatus::Running,
        channels,
        version: 0,
        checkpoint_id: None,
        created_at: "0".to_owned(),
        updated_at: "0".to_owned(),
    }
}

/// Build the gateway that backs a prebuilt agent, mirroring the napi bridge's
/// `build_gateway`: register the adapter matching the RESOLVED provider when its
/// credentials are present in env. Missing credentials fail loud; the deterministic mock
/// answers only for the mock provider or in offline mode (`AILU_LLM_MOCK=1`).
fn build_gateway(resolved: &ModelChoice) -> Result<Arc<DefaultLlmGateway>, String> {
    let mut gateway = DefaultLlmGateway::new();

    let model = if resolved.model.is_empty() {
        None
    } else {
        Some(resolved.model.clone())
    };
    let key = provider_key_from_env(resolved.provider);

    let registered = match resolved.provider {
        LlmProvider::Mistral => key.map(|key| {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::mistral(
                Some(key),
                model.clone(),
            )));
        }),
        LlmProvider::Openai => key.map(|key| {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::openai(
                Some(key),
                model.clone(),
            )));
        }),
        LlmProvider::Openrouter => key.map(|key| {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::openrouter(
                Some(key),
                model.clone(),
            )));
        }),
        LlmProvider::Minimax => key.map(|key| {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::minimax(
                Some(key),
                model.clone(),
            )));
        }),
        LlmProvider::Huggingface => key.map(|key| {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::huggingface(
                Some(key),
                model.clone(),
            )));
        }),
        LlmProvider::Anthropic => key.map(|key| {
            gateway.register_adapter(Box::new(AnthropicAdapter::new(Box::new(
                HttpAnthropicPort::new(key),
            ))));
        }),
        LlmProvider::Google => key.map(|key| {
            gateway.register_adapter(Box::new(GeminiAdapter::new(Box::new(HttpGeminiPort::new(
                key,
            )))));
        }),
        LlmProvider::Ollama if std::env::var("AILU_USE_OLLAMA").as_deref() == Ok("1") => {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::ollama(
                model.clone(),
                None,
            )));
            Some(())
        }
        LlmProvider::Lmstudio if std::env::var("AILU_USE_LMSTUDIO").as_deref() == Ok("1") => {
            gateway.register_adapter(Box::new(OpenAiCompatibleAdapter::lmstudio(
                model.clone(),
                None,
            )));
            Some(())
        }
        // `Mock`, or a real provider whose env credentials are missing.
        _ => None,
    };

    if registered.is_none() {
        if !offline_mock_enabled() {
            return Err(missing_credentials_message(resolved.provider));
        }
        gateway.register_adapter(Box::new(mock_adapter(resolved.provider)));
    }

    Ok(Arc::new(gateway))
}

/// A deterministic mock that immediately finalizes a short answer. Registered under
/// the resolved provider slot so the agent's request always finds an adapter.
fn mock_adapter(provider: LlmProvider) -> MockAdapter {
    let response = LlmResponse {
        content: "FINAL: done".to_owned(),
        tool_calls: None,
        stop_reason: Some("end_turn".to_owned()),
        usage: LlmUsage::default(),
        model: "mock".to_owned(),
        provider,
        content_blocks: None,
    };
    MockAdapter::new(provider, vec![response])
}

/// A `replace`-reducer JSON channel definition.
fn replace_channel() -> ChannelDefinition {
    ChannelDefinition {
        channel_type: "json".to_owned(),
        reducer: ChannelReducer::Replace,
        default: None,
        no_log: false,
    }
}

/// An agent node definition with the given id.
fn agent_node(id: &str) -> NodeDefinition {
    NodeDefinition {
        id: NodeId::from(id),
        node_type: NodeType::Agent,
        label: id.to_owned(),
        subgraph_id: None,
        input_mapping: None,
        output_mapping: None,
        fan_out: None,
        map_subgraph: None,
        retry_policy: None,
        metadata: None,
    }
}

/// Drive a future to completion on a fresh current-thread tokio runtime. There are
/// no Python callbacks in any of these run paths, so blocking the calling thread is
/// safe (no deadlock risk).
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread tokio runtime")
        .block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-wide lock serialising the env-mutating tests in this module so they
    /// cannot race other env-reading tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn restore_env(key: &str, prev: Option<String>) {
        match prev {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    /// `resolve_model` round-trips: a `fast` tier against an explicit `["mistral"]`
    /// available list resolves to the mistral column, and the returned JSON
    /// deserialises back to the same `ModelChoice` (camelCase wire shape).
    #[test]
    fn resolve_model_json_round_trip() {
        let json = resolve_model("fast", Some("[\"mistral\"]"), None).expect("resolves");

        // Wire shape is camelCase and matches the policy's mistral/fast cell.
        assert!(json.contains("\"provider\":\"mistral\""));
        assert!(json.contains("\"model\":\"mistral-small-latest\""));
        assert!(json.contains("\"recommended\":true"));

        let choice: ModelChoice = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(choice.provider, LlmProvider::Mistral);
        assert_eq!(choice.model, "mistral-small-latest");
        assert!(choice.recommended);
    }

    /// An override `{ provider, model }` wins and is flagged non-recommended.
    #[test]
    fn resolve_model_honours_override() {
        let json = resolve_model(
            "frontier",
            Some("[\"anthropic\"]"),
            Some("{\"provider\":\"mistral\",\"model\":\"mistral-tiny\"}"),
        )
        .expect("resolves");
        let choice: ModelChoice = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(choice.provider, LlmProvider::Mistral);
        assert_eq!(choice.model, "mistral-tiny");
        assert!(!choice.recommended);
    }

    /// An unknown tier is rejected at the boundary.
    #[test]
    fn resolve_model_rejects_unknown_tier() {
        assert!(resolve_model("turbo", None, None).is_err());
    }

    /// `list_components` exposes the registry kinds; `list_prebuilt` the 16 agents.
    #[test]
    fn catalogs_expose_kinds_and_agents() {
        let kinds: Vec<String> =
            serde_json::from_str(&list_components().expect("kinds")).expect("array");
        assert!(kinds.contains(&"promptBuilder".to_owned()));
        assert_eq!(kinds.len(), ComponentRegistry::kinds().len());

        let agents: Vec<PrebuiltAgent> =
            serde_json::from_str(&list_prebuilt().expect("agents")).expect("array");
        assert_eq!(agents.len(), 16);
        assert!(agents.iter().any(|a| a.name == "summarizer"));
    }

    /// `run_component(promptBuilder)` renders its template against the channels and
    /// returns the channel-update map (`{ "prompt": "Hello Ada!" }`).
    #[test]
    fn run_component_prompt_builder_produces_channel_update() {
        let update = run_component(
            "promptBuilder",
            "{\"template\":\"Hello {{name}}!\",\"into\":\"prompt\"}",
            "{\"name\":\"Ada\"}",
        )
        .expect("runs");
        let map: BTreeMap<String, Value> = serde_json::from_str(&update).expect("object");
        assert_eq!(map.get("prompt"), Some(&json!("Hello Ada!")));
    }

    /// An unknown component kind is rejected at build time.
    #[test]
    fn run_component_unknown_kind_errors() {
        assert!(run_component("nope", "{}", "{}").is_err());
    }

    /// `run_prebuilt(summarizer)` with no provider env fails loud, and in offline mode
    /// (`AILU_LLM_MOCK=1`) completes on the mock gateway: the run reaches `completed`, the
    /// agent's `summary` channel is populated, and the resolved provider falls back to `mock`.
    #[test]
    fn run_prebuilt_summarizer_completes_on_mock() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_mistral = std::env::var("MISTRAL_API_KEY").ok();
        let prev_anthropic = std::env::var("ANTHROPIC_API_KEY").ok();
        let prev_ollama = std::env::var("AILU_USE_OLLAMA").ok();

        let prev_offline = std::env::var("AILU_LLM_MOCK").ok();

        // Force-unset every provider so the policy resolves to the mock. Without offline mode
        // that fails loud; with it the gateway registers the deterministic mock adapter.
        std::env::remove_var("MISTRAL_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("AILU_USE_OLLAMA");
        std::env::remove_var("AILU_LLM_MOCK");
        let keyless = run_prebuilt("summarizer", "\"please summarise this text\"", None);
        std::env::set_var("AILU_LLM_MOCK", "1");

        let outcome = run_prebuilt("summarizer", "\"please summarise this text\"", None);
        restore_env("AILU_LLM_MOCK", prev_offline);
        let outcome = outcome.expect("runs");
        let keyless = keyless.expect_err("a keyless run fails loud");
        assert!(keyless.contains("AILU_LLM_MOCK=1"), "{keyless}");

        let value: Value = serde_json::from_str(&outcome).expect("object");
        assert_eq!(
            value.get("status").and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(
            value
                .pointer("/resolvedModel/provider")
                .and_then(Value::as_str),
            Some("mock")
        );
        // The summarizer writes into its `summary` output channel.
        assert!(value.pointer("/channels/summary").is_some());

        restore_env("MISTRAL_API_KEY", prev_mistral);
        restore_env("ANTHROPIC_API_KEY", prev_anthropic);
        restore_env("AILU_USE_OLLAMA", prev_ollama);
    }
}
