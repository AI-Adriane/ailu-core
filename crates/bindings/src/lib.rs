//! Node bindings (napi-rs) over the Rust engine.
//!
//! Two layers:
//!
//! 1. **Sync JSON helpers** (`validate_graph_json`, `compile_graph_yaml_json`,
//!    `engine_version`) — JSON in / JSON out, no callbacks. The original surface,
//!    kept working.
//! 2. **The async run bridge** (`engine_run` / `engine_resume` /
//!    `engine_approve_and_resume`) — drives a Rust [`GraphRuntime`] from a JS caller
//!    while calling user-supplied JS closures back from Rust:
//!      * custom node handlers (`on_node` with `{ kind: "node", ... }`),
//!      * tool `execute` fns (`on_node` with `{ kind: "tool", ... }`),
//!      * named condition predicates (`on_condition`),
//!      * a fire-and-forget run-event sink (`on_event`).
//!
//!    These entry points are **async** napi fns: each returns a JS `Promise`, so the
//!    event loop stays free to service the [`ThreadsafeFunction`] callbacks while the
//!    Rust run future is parked on a oneshot. A synchronous fn would block the JS
//!    main thread and deadlock (the callbacks could never run). The run future is
//!    `Send` (see `graph-runtime`), which is what lets napi's tokio runtime drive it.
//!
//! The JS seam callbacks are **async**: `on_node` and `on_condition` return a
//! `Promise`, and Rust *awaits* it. `ThreadsafeFunction::call_async` (napi 3 takes the
//! `Return = Promise<String>` from the TSFN generics, not a turbofish) resolves the
//! synchronously-returned promise object, and a further `.await` drives that promise to
//! its JS-resolved value (napi's `Promise<T>` implements both `FromNapiValue` and
//! `Future`). So `on_node` resolves to a JSON string and
//! `on_condition` to a boolean-ish JSON string, each *after* any JS `await` inside
//! the callback. `on_event` stays fire-and-forget (no return awaited).

#![deny(clippy::all)]

use std::collections::BTreeMap;
use std::sync::Arc;

use ailu_graph_ailu::compile_graph_yaml;
use ailu_graph_core::{validate_graph, GraphDefinition};
use ailu_llm_gateway::{LlmGateway, LlmRequest};
use ailu_runtime_bridge::{BridgeResult, Entry, HostCallbacks};
use async_trait::async_trait;
use napi::bindgen_prelude::Promise;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Status;
use napi_derive::napi;
use serde_json::Value;

/// A JS callback taking one string argument. Declared as a napi `ThreadsafeFunction`
/// (which is `Send + Sync`) so the async run future can capture it across `.await`
/// points — a bare `JsFunction` is `!Send` and would make the future non-`Send`.
///
/// napi 3 removed `ErrorStrategy`; the napi-2 `Fatal` behavior is now the
/// `CalleeHandled = false` const-generic (the 5th type param): JS receives
/// `(payloadString)` with no leading error argument, `.call(...)` takes a bare
/// `String` (not a `Result`), and a JS throw surfaces as a fatal napi exception.
/// `Return = Promise<String>` is what the async seams (`on_node`/`on_condition`)
/// await; `on_event` is fire-and-forget and never awaits it. `CallJsBackArgs`
/// stays the default `T` (`String`) so the napi-generated marshalling passes the
/// payload as a single JS string argument.
type StringCallback = ThreadsafeFunction<String, Promise<String>, String, Status, false>;

#[derive(Clone)]
struct NapiCallbacks {
    on_node: Arc<StringCallback>,
    on_condition: Arc<StringCallback>,
    on_event: Arc<StringCallback>,
    /// Cooperative cancellation (ADR 0044). OPTIONAL: a JS caller built against the previous
    /// signature passes only three callbacks, and the run then behaves exactly as before
    /// (never cancelled). Reuses the awaited, value-returning shape of `on_condition` —
    /// `on_event` is fire-and-forget and structurally cannot answer a question.
    is_cancelled: Option<Arc<StringCallback>>,
}

impl NapiCallbacks {
    fn new(
        on_node: StringCallback,
        on_condition: StringCallback,
        on_event: StringCallback,
        is_cancelled: Option<StringCallback>,
    ) -> Self {
        Self {
            on_node: Arc::new(on_node),
            on_condition: Arc::new(on_condition),
            on_event: Arc::new(on_event),
            is_cancelled: is_cancelled.map(Arc::new),
        }
    }
}

#[async_trait]
impl HostCallbacks for NapiCallbacks {
    async fn on_node(&self, payload: Value) -> BridgeResult<String> {
        call_js_string(&self.on_node, payload).await
    }

    fn on_condition(&self, payload: Value) -> BridgeResult<bool> {
        call_js_bool_awaiting(&self.on_condition, payload)
    }

    fn on_event(&self, payload_json: String) {
        let _ = self
            .on_event
            .call(payload_json, ThreadsafeFunctionCallMode::NonBlocking);
    }

    /// Ask JS whether this run should stop. Absent callback (an older caller) or a failed
    /// call both read as "not cancelled": cancellation must never be INVENTED by a transport
    /// hiccup — the run continues and the embedder can ask again at the next boundary.
    fn is_cancelled(&self) -> bool {
        self.is_cancelled
            .as_ref()
            .is_some_and(|cb| call_js_bool_awaiting(cb, Value::Null).unwrap_or(false))
    }
}

fn to_napi(error: String) -> napi::Error {
    napi::Error::from_reason(error)
}

async fn call_js_string(tsfn: &StringCallback, payload: Value) -> BridgeResult<String> {
    let promise: Promise<String> = tsfn
        .call_async(payload.to_string())
        .await
        .map_err(|error| error.to_string())?;
    promise.await.map_err(|error| error.to_string())
}

fn call_js_bool_awaiting(tsfn: &Arc<StringCallback>, payload: Value) -> BridgeResult<bool> {
    let tsfn = Arc::clone(tsfn);
    let (tx, rx) = std::sync::mpsc::sync_channel::<BridgeResult<String>>(1);
    tokio::task::block_in_place(move || {
        napi::bindgen_prelude::spawn(async move {
            let result = call_js_string(&tsfn, payload).await;
            let _ = tx.send(result);
        });
        rx.recv()
            .map_err(|_| "condition callback dropped without a value".to_owned())?
            .map(|text| ailu_runtime_bridge::parse_bool(&text))
    })
}

/// Version of the bound Rust engine.
#[napi]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Validate a graph definition (JSON). Returns a JSON array of validation errors —
/// empty when the graph is structurally sound. Errors on malformed JSON.
#[napi]
pub fn validate_graph_json(definition_json: String) -> napi::Result<String> {
    let definition: GraphDefinition = serde_json::from_str(&definition_json)
        .map_err(|error| napi::Error::from_reason(format!("invalid graph JSON: {error}")))?;
    let errors = validate_graph(&definition);
    serde_json::to_string(&errors).map_err(|error| napi::Error::from_reason(error.to_string()))
}

/// Compile graph DSL YAML into a validated `GraphDefinition` (JSON string).
/// Throws with a clear message on parse, DSL, or structural validation failure.
#[napi]
pub fn compile_graph_yaml_json(yaml: String) -> napi::Result<String> {
    let definition =
        compile_graph_yaml(&yaml).map_err(|error| napi::Error::from_reason(error.to_string()))?;
    serde_json::to_string(&definition).map_err(|error| napi::Error::from_reason(error.to_string()))
}

/// The optional custom-endpoint field the SDK sends alongside a standalone `LlmRequest`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StandaloneEndpoint {
    #[serde(default)]
    base_url: Option<String>,
}

/// One-shot LLM completion over the Rust gateway (ADR 0031 — backs the SDK `Model.invoke()`
/// overlay). `request_json` is a serialized `LlmRequest` (provider / model / messages / …);
/// `provider_keys_json` is a `{ "<provider>": "<key>" }` map (may be `"{}"` → env keys, else a
/// deterministic mock). Resolves to a serialized `LlmResponse`. The HTTP happens in Rust — no
/// TS provider client, one engine.
#[napi(ts_return_type = "Promise<string>")]
pub async fn llm_complete(
    request_json: String,
    provider_keys_json: String,
) -> napi::Result<String> {
    let request: LlmRequest = serde_json::from_str(&request_json)
        .map_err(|error| napi::Error::from_reason(format!("invalid LLM request JSON: {error}")))?;
    let keys: BTreeMap<String, String> =
        serde_json::from_str(&provider_keys_json).map_err(|error| {
            napi::Error::from_reason(format!("invalid provider keys JSON: {error}"))
        })?;
    let model = if request.model.is_empty() {
        None
    } else {
        Some(request.model.clone())
    };
    // `model.openaiCompatible({ baseURL })`: the SDK adds `baseUrl` next to the `LlmRequest`
    // fields. Such a request goes to that endpoint only, with the key the SDK resolved for it
    // (its `apiKeyEnv`) — never to the provider's public API with the provider's key.
    let base_url = serde_json::from_str::<StandaloneEndpoint>(&request_json)
        .ok()
        .and_then(|endpoint| endpoint.base_url)
        .filter(|url| !url.trim().is_empty());
    let gateway = match base_url {
        Some(base_url) => {
            let slug = serde_json::to_value(request.provider)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            ailu_runtime_bridge::build_standalone_custom_endpoint_gateway(
                &base_url,
                request.provider,
                keys.get(&slug).cloned(),
                model,
            )
            .map_err(napi::Error::from_reason)?
        }
        None => ailu_runtime_bridge::build_standalone_gateway(request.provider, model, &keys)
            .map_err(napi::Error::from_reason)?,
    };
    let response = gateway
        .complete(request)
        .await
        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
    serde_json::to_string(&response).map_err(|error| napi::Error::from_reason(error.to_string()))
}

/// Start a fresh run of a graph on the Rust engine.
///
/// `spec_json` is an [`ailu_runtime_bridge::spec::EngineSpec`]: the graph, the run id, optional
/// initial channel data, the agent configs, and the ids/names whose handlers live
/// in JS. The three callbacks bridge back to JS:
/// - `on_node(payloadJson) -> Promise<updateJson>` for JS node handlers and JS tools,
/// - `on_condition(payloadJson) -> Promise<"true"|"false">` for named conditions,
/// - `on_event(payloadJson)` (fire-and-forget) for run-lifecycle events,
/// - `is_cancelled()` (OPTIONAL, ADR 0044) polled at every node boundary — `"true"` stops the
///   run cleanly with status `cancelled` after its last checkpoint. Omit it for the previous
///   behaviour (a run that can only end by completing, suspending or failing).
///
/// Resolves to a JSON [`ailu_runtime_bridge::spec::RunOutcome`] (final state + any pending
/// approvals + the serialized state needed for `engine_approve_and_resume`).
#[napi(
    ts_args_type = "specJson: string, onNode: (payloadJson: string) => string | Promise<string>, onCondition: (payloadJson: string) => boolean | string | Promise<boolean | string>, onEvent: (payloadJson: string) => void, isCancelled?: (payloadJson: string) => boolean | string | Promise<boolean | string>",
    ts_return_type = "Promise<string>"
)]
pub async fn engine_run(
    spec_json: String,
    on_node: StringCallback,
    on_condition: StringCallback,
    on_event: StringCallback,
    is_cancelled: Option<StringCallback>,
) -> napi::Result<String> {
    let callbacks = Arc::new(NapiCallbacks::new(
        on_node,
        on_condition,
        on_event,
        is_cancelled,
    ));
    ailu_runtime_bridge::run(spec_json, callbacks, Entry::Start)
        .await
        .map_err(to_napi)
}

/// Resume a previously suspended run from its serialized state (carried in
/// `spec_json.state`). Same callbacks as [`engine_run`].
#[napi(
    ts_args_type = "specJson: string, onNode: (payloadJson: string) => string | Promise<string>, onCondition: (payloadJson: string) => boolean | string | Promise<boolean | string>, onEvent: (payloadJson: string) => void, isCancelled?: (payloadJson: string) => boolean | string | Promise<boolean | string>",
    ts_return_type = "Promise<string>"
)]
pub async fn engine_resume(
    spec_json: String,
    on_node: StringCallback,
    on_condition: StringCallback,
    on_event: StringCallback,
    is_cancelled: Option<StringCallback>,
) -> napi::Result<String> {
    let callbacks = Arc::new(NapiCallbacks::new(
        on_node,
        on_condition,
        on_event,
        is_cancelled,
    ));
    ailu_runtime_bridge::run(spec_json, callbacks, Entry::Resume)
        .await
        .map_err(to_napi)
}

/// Grant the approved tools carried in `spec_json.approvedTools`, write them into
/// the resumed state's `__approvedTools` channel, then resume. Same callbacks as
/// [`engine_run`].
#[napi(
    ts_args_type = "specJson: string, onNode: (payloadJson: string) => string | Promise<string>, onCondition: (payloadJson: string) => boolean | string | Promise<boolean | string>, onEvent: (payloadJson: string) => void, isCancelled?: (payloadJson: string) => boolean | string | Promise<boolean | string>",
    ts_return_type = "Promise<string>"
)]
pub async fn engine_approve_and_resume(
    spec_json: String,
    on_node: StringCallback,
    on_condition: StringCallback,
    on_event: StringCallback,
    is_cancelled: Option<StringCallback>,
) -> napi::Result<String> {
    let callbacks = Arc::new(NapiCallbacks::new(
        on_node,
        on_condition,
        on_event,
        is_cancelled,
    ));
    ailu_runtime_bridge::run(spec_json, callbacks, Entry::Approve)
        .await
        .map_err(to_napi)
}

/// Deliver an external signal to a suspended run, then resume it. `signalName` is the
/// signal a `waitForSignal` node is blocked on; `payloadJson` is its payload (injected
/// into the run's `__signals[signalName]` channel). The run advances PAST the waiting
/// node. `specJson.state` carries the serialized suspended `GraphState`; callbacks are
/// the same as [`engine_run`].
#[napi(
    ts_args_type = "specJson: string, signalName: string, payloadJson: string, onNode: (payloadJson: string) => string | Promise<string>, onCondition: (payloadJson: string) => boolean | string | Promise<boolean | string>, onEvent: (payloadJson: string) => void, isCancelled?: (payloadJson: string) => boolean | string | Promise<boolean | string>",
    ts_return_type = "Promise<string>"
)]
pub async fn engine_signal(
    spec_json: String,
    signal_name: String,
    payload_json: String,
    on_node: StringCallback,
    on_condition: StringCallback,
    on_event: StringCallback,
    is_cancelled: Option<StringCallback>,
) -> napi::Result<String> {
    let payload: Value = serde_json::from_str(&payload_json).map_err(|error| {
        napi::Error::from_reason(format!("invalid signal payload JSON: {error}"))
    })?;
    let callbacks = Arc::new(NapiCallbacks::new(
        on_node,
        on_condition,
        on_event,
        is_cancelled,
    ));
    ailu_runtime_bridge::run(
        spec_json,
        callbacks,
        Entry::Signal {
            name: signal_name,
            payload,
        },
    )
    .await
    .map_err(to_napi)
}

/// Replay-as-evidence (ADR 0038): re-execute a run from `checkpointId`, re-feeding the
/// recorded LLM outputs + timestamps from `specJson.replayJournal` instead of re-sampling.
/// Returns the replayed run's `RunOutcome` JSON (a forked, read-only re-derivation). The
/// caller compares the replayed governance decisions to the attested chain (verify-replay).
#[napi(
    ts_args_type = "specJson: string, checkpointId: string, onNode: (payloadJson: string) => string | Promise<string>, onCondition: (payloadJson: string) => boolean | string | Promise<boolean | string>, onEvent: (payloadJson: string) => void",
    ts_return_type = "Promise<string>"
)]
pub async fn engine_replay(
    spec_json: String,
    checkpoint_id: String,
    on_node: StringCallback,
    on_condition: StringCallback,
    on_event: StringCallback,
) -> napi::Result<String> {
    // ADR 0044: a replay is a deterministic RE-DERIVATION of a run that already happened, so it
    // deliberately takes no cancellation seam — a live cancel flag must never change what a
    // replay reproduces, or verify-replay would stop being evidence.
    let callbacks = Arc::new(NapiCallbacks::new(on_node, on_condition, on_event, None));
    ailu_runtime_bridge::run(spec_json, callbacks, Entry::Replay { checkpoint_id })
        .await
        .map_err(to_napi)
}
