//! Stable C ABI over the Ailu Rust engine.
//!
//! The contract is deliberately small and language-neutral: UTF-8 C strings in,
//! owned UTF-8 C strings out, and one explicit free function. Higher-level SDKs
//! should keep their ergonomic builders locally, then cross this boundary with
//! JSON/YAML documents so every language uses the same Rust validator/compiler.

#![deny(clippy::all)]

use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::OnceLock;

use ailu_runtime_bridge::{BridgeResult, Entry, HostCallbacks, SharedCallbacks};
use ailu_sdk_core as core;
use async_trait::async_trait;
use serde_json::Value;
use tokio::runtime::{Builder, Runtime};

/// Call completed successfully.
pub const AILU_OK: c_int = 0;
/// The caller passed a null pointer.
pub const AILU_ERR_NULL: c_int = 1;
/// The caller passed bytes that are not valid UTF-8.
pub const AILU_ERR_UTF8: c_int = 2;
/// The caller passed malformed JSON/YAML or the engine rejected the document.
pub const AILU_ERR_INPUT: c_int = 3;
/// The engine produced a value that could not be serialized at the C boundary.
pub const AILU_ERR_INTERNAL: c_int = 4;

/// Result returned by every fallible Ailu C-ABI function.
///
/// On success, `code == AILU_OK`, `value` points to an owned null-terminated
/// UTF-8 string, and `error == NULL`.
///
/// On failure, `value == NULL` and `error` points to an owned null-terminated
/// UTF-8 string. The caller must release the returned allocation with
/// `ailu_result_free` or `ailu_string_free`.
#[repr(C)]
pub struct AiluResult {
    pub code: c_int,
    pub value: *mut c_char,
    pub error: *mut c_char,
}

impl AiluResult {
    fn ok(value: String) -> Self {
        AiluResult {
            code: AILU_OK,
            value: into_c_string(value),
            error: ptr::null_mut(),
        }
    }

    fn err(code: c_int, error: impl Into<String>) -> Self {
        AiluResult {
            code,
            value: ptr::null_mut(),
            error: into_c_string(error.into()),
        }
    }
}

pub type AiluStringCallback = Option<
    unsafe extern "C" fn(
        payload_json: *const c_char,
        user_data: *mut c_void,
        value: *mut *const c_char,
        error: *mut *const c_char,
    ) -> c_int,
>;
pub type AiluEventCallback =
    Option<unsafe extern "C" fn(payload_json: *const c_char, user_data: *mut c_void)>;

#[repr(C)]
pub struct AiluCallbacks {
    pub user_data: *mut c_void,
    pub on_node: AiluStringCallback,
    pub on_condition: AiluStringCallback,
    pub on_event: AiluEventCallback,
}

#[derive(Clone, Copy)]
struct CCallbacks {
    user_data: usize,
    on_node: AiluStringCallback,
    on_condition: AiluStringCallback,
    on_event: AiluEventCallback,
}

unsafe impl Send for CCallbacks {}
unsafe impl Sync for CCallbacks {}

impl From<AiluCallbacks> for CCallbacks {
    fn from(callbacks: AiluCallbacks) -> Self {
        Self {
            user_data: callbacks.user_data as usize,
            on_node: callbacks.on_node,
            on_condition: callbacks.on_condition,
            on_event: callbacks.on_event,
        }
    }
}

#[async_trait]
impl HostCallbacks for CCallbacks {
    async fn on_node(&self, payload: Value) -> BridgeResult<String> {
        self.call_string(self.on_node, payload.to_string(), "on_node")
    }

    fn on_condition(&self, payload: Value) -> BridgeResult<bool> {
        self.call_string(self.on_condition, payload.to_string(), "on_condition")
            .map(|text| ailu_runtime_bridge::parse_bool(&text))
    }

    fn on_event(&self, payload_json: String) {
        let Some(callback) = self.on_event else {
            return;
        };
        let payload = CString::new(payload_json.replace('\0', "\\0"))
            .expect("internal NULs were escaped before building CString");
        unsafe {
            callback(payload.as_ptr(), self.user_data as *mut c_void);
        }
    }
}

impl CCallbacks {
    fn call_string(
        &self,
        callback: AiluStringCallback,
        payload_json: String,
        name: &str,
    ) -> BridgeResult<String> {
        let callback = callback.ok_or_else(|| format!("{name} callback is null"))?;
        let payload = CString::new(payload_json.replace('\0', "\\0"))
            .expect("internal NULs were escaped before building CString");
        let mut value = ptr::null();
        let mut error = ptr::null();
        let code = unsafe {
            callback(
                payload.as_ptr(),
                self.user_data as *mut c_void,
                &mut value,
                &mut error,
            )
        };
        copy_callback_result(code, value, error, name)
    }
}

fn copy_callback_result(
    code: c_int,
    value: *const c_char,
    error: *const c_char,
    name: &str,
) -> BridgeResult<String> {
    if code == AILU_OK {
        if value.is_null() {
            return Err(format!("{name} callback returned null value"));
        }
        return unsafe { borrowed_c_str(value) }
            .map(str::to_owned)
            .map_err(|error| format!("{name} callback returned invalid value: {error}"));
    }

    let message = if error.is_null() {
        format!("{name} callback failed with code {code}")
    } else {
        unsafe { borrowed_c_str(error) }
            .map(str::to_owned)
            .unwrap_or_else(|error| format!("{name} callback error was invalid UTF-8: {error}"))
    };
    Err(message)
}

/// Version of the bound Rust engine.
///
/// The returned string is owned by the caller and must be released with
/// `ailu_string_free`.
#[no_mangle]
pub extern "C" fn ailu_engine_version() -> *mut c_char {
    into_c_string(core::engine_version())
}

/// Validate a graph definition JSON document.
///
/// Returns a JSON array of validation errors; an empty array means the graph is
/// structurally sound.
///
/// # Safety
///
/// `definition_json` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_validate_graph_json(definition_json: *const c_char) -> AiluResult {
    unsafe {
        with_c_str(definition_json, |raw| {
            core::validate_graph_json(raw).map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
}

/// Compile Ailu graph DSL YAML into a validated `GraphDefinition` JSON document.
///
/// # Safety
///
/// `yaml` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_compile_graph_yaml_json(yaml: *const c_char) -> AiluResult {
    unsafe {
        with_c_str(yaml, |raw| {
            core::compile_graph_yaml(raw).map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
}

/// Return the providers usable in the current process env as a JSON array.
#[no_mangle]
pub extern "C" fn ailu_available_providers_json() -> AiluResult {
    from_core(core::available_providers())
}

/// Resolve a capability tier to a concrete model choice JSON document.
///
/// `available_json` may be `NULL`; when present it must be a JSON array of
/// provider strings. `override_json` may be `NULL`; when present it must be
/// `{ "provider"?: string, "model"?: string }`.
///
/// # Safety
///
/// `tier` must be a valid, null-terminated UTF-8 C string pointer. Optional
/// pointers must be either `NULL` or valid null-terminated UTF-8 C strings.
#[no_mangle]
pub unsafe extern "C" fn ailu_resolve_model_json(
    tier: *const c_char,
    available_json: *const c_char,
    override_json: *const c_char,
) -> AiluResult {
    let tier = match unsafe { read_required_c_str(tier) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let available = match unsafe { read_optional_c_str(available_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let override_ = match unsafe { read_optional_c_str(override_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };

    from_core(core::resolve_model(tier, available, override_))
}

/// Return every native component kind as a JSON array.
#[no_mangle]
pub extern "C" fn ailu_list_components_json() -> AiluResult {
    from_core(core::list_components())
}

/// Return every prebuilt micro-agent definition as JSON.
#[no_mangle]
pub extern "C" fn ailu_list_prebuilt_json() -> AiluResult {
    from_core(core::list_prebuilt())
}

/// Run a native component handler fully on Rust.
///
/// # Safety
///
/// All pointers must be valid, null-terminated UTF-8 C strings.
#[no_mangle]
pub unsafe extern "C" fn ailu_run_component_json(
    kind: *const c_char,
    params_json: *const c_char,
    channels_json: *const c_char,
) -> AiluResult {
    let kind = match unsafe { read_required_c_str(kind) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let params = match unsafe { read_required_c_str(params_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let channels = match unsafe { read_required_c_str(channels_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };

    from_core(core::run_component(kind, params, channels))
}

/// Run a prebuilt micro-agent fully on Rust.
///
/// `options_json` may be `NULL`; when present it must be
/// `{ "provider"?: string, "model"?: string }`.
///
/// # Safety
///
/// Required pointers must be valid, null-terminated UTF-8 C strings. The optional
/// pointer must be either `NULL` or a valid null-terminated UTF-8 C string.
#[no_mangle]
pub unsafe extern "C" fn ailu_run_prebuilt_json(
    name: *const c_char,
    input_json: *const c_char,
    options_json: *const c_char,
) -> AiluResult {
    let name = match unsafe { read_required_c_str(name) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let input = match unsafe { read_required_c_str(input_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };
    let options = match unsafe { read_optional_c_str(options_json) } {
        Ok(value) => value,
        Err(result) => return result,
    };

    from_core(core::run_prebuilt(name, input, options))
}

/// Start a callback-capable engine run from an EngineSpec JSON document.
///
/// The spec wire shape is the same one used by the TypeScript N-API bridge.
/// Callback pointers may be invoked from runtime worker threads and must remain
/// valid until this function returns.
///
/// # Safety
///
/// `spec_json` must be a valid, null-terminated UTF-8 C string pointer. Callback
/// function pointers, when present, must be valid for the full duration of this call.
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_run_json(
    spec_json: *const c_char,
    callbacks: AiluCallbacks,
) -> AiluResult {
    unsafe { run_engine_entry(spec_json, callbacks, Entry::Start) }
}

/// Resume a callback-capable run from `spec_json.state`.
///
/// # Safety
///
/// Same requirements as [`ailu_engine_run_json`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_resume_json(
    spec_json: *const c_char,
    callbacks: AiluCallbacks,
) -> AiluResult {
    unsafe { run_engine_entry(spec_json, callbacks, Entry::Resume) }
}

/// Approve host-provided tools carried in `spec_json.approvedTools`, then resume.
///
/// # Safety
///
/// Same requirements as [`ailu_engine_run_json`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_approve_and_resume_json(
    spec_json: *const c_char,
    callbacks: AiluCallbacks,
) -> AiluResult {
    unsafe { run_engine_entry(spec_json, callbacks, Entry::Approve) }
}

/// Deliver an external signal payload, then resume the suspended run.
///
/// # Safety
///
/// All string pointers must be valid, null-terminated UTF-8 C strings. Callback
/// function pointers, when present, must be valid for the full duration of this call.
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_signal_json(
    spec_json: *const c_char,
    signal_name: *const c_char,
    payload_json: *const c_char,
    callbacks: AiluCallbacks,
) -> AiluResult {
    let name = match unsafe { read_required_c_str(signal_name) } {
        Ok(value) => value.to_owned(),
        Err(result) => return result,
    };
    let payload = match unsafe { read_required_c_str(payload_json) } {
        Ok(value) => match serde_json::from_str::<Value>(value) {
            Ok(payload) => payload,
            Err(error) => {
                return AiluResult::err(
                    AILU_ERR_INPUT,
                    format!("invalid signal payload JSON: {error}"),
                )
            }
        },
        Err(result) => return result,
    };
    unsafe { run_engine_entry(spec_json, callbacks, Entry::Signal { name, payload }) }
}

/// Replay a recorded run from `checkpoint_id`.
///
/// # Safety
///
/// Same requirements as [`ailu_engine_run_json`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_replay_json(
    spec_json: *const c_char,
    checkpoint_id: *const c_char,
    callbacks: AiluCallbacks,
) -> AiluResult {
    let checkpoint_id = match unsafe { read_required_c_str(checkpoint_id) } {
        Ok(value) => value.to_owned(),
        Err(result) => return result,
    };
    unsafe { run_engine_entry(spec_json, callbacks, Entry::Replay { checkpoint_id }) }
}

/// Free a string returned by the Ailu C ABI.
///
/// Passing `NULL` is allowed.
///
/// # Safety
///
/// `ptr` must be either `NULL` or a pointer previously returned by the Ailu C
/// ABI that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn ailu_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(ptr));
    }
}

/// Free both string fields carried by an `AiluResult`.
///
/// Passing a zeroed or already-empty result is allowed. Do not use the pointers
/// after calling this function.
///
/// # Safety
///
/// Any non-null pointer in `result` must have been returned by the Ailu C ABI
/// and must not already have been freed.
#[no_mangle]
pub unsafe extern "C" fn ailu_result_free(result: AiluResult) {
    unsafe {
        ailu_string_free(result.value);
        ailu_string_free(result.error);
    }
}

unsafe fn with_c_str(
    input: *const c_char,
    f: impl FnOnce(&str) -> Result<String, (c_int, String)>,
) -> AiluResult {
    let input = match unsafe { read_required_c_str(input) } {
        Ok(input) => input,
        Err(result) => return result,
    };

    match f(input) {
        Ok(value) => AiluResult::ok(value),
        Err((code, error)) => AiluResult::err(code, error),
    }
}

unsafe fn run_engine_entry(
    spec_json: *const c_char,
    callbacks: AiluCallbacks,
    entry: Entry,
) -> AiluResult {
    let spec = match unsafe { read_required_c_str(spec_json) } {
        Ok(value) => value.to_owned(),
        Err(result) => return result,
    };
    let callbacks: SharedCallbacks = std::sync::Arc::new(CCallbacks::from(callbacks));
    match runtime().block_on(ailu_runtime_bridge::run(spec, callbacks, entry)) {
        Ok(value) => AiluResult::ok(value),
        Err(error) => AiluResult::err(AILU_ERR_INPUT, error),
    }
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to initialize Ailu C ABI runtime")
    })
}

unsafe fn read_required_c_str<'a>(input: *const c_char) -> Result<&'a str, AiluResult> {
    if input.is_null() {
        return Err(AiluResult::err(
            AILU_ERR_NULL,
            "input pointer must not be null",
        ));
    }

    unsafe { CStr::from_ptr(input) }.to_str().map_err(|error| {
        AiluResult::err(AILU_ERR_UTF8, format!("input is not valid UTF-8: {error}"))
    })
}

unsafe fn borrowed_c_str<'a>(input: *const c_char) -> Result<&'a str, std::str::Utf8Error> {
    unsafe { CStr::from_ptr(input) }.to_str()
}

unsafe fn read_optional_c_str<'a>(input: *const c_char) -> Result<Option<&'a str>, AiluResult> {
    if input.is_null() {
        return Ok(None);
    }
    unsafe { read_required_c_str(input) }.map(Some)
}

fn from_core(result: Result<String, String>) -> AiluResult {
    match result {
        Ok(value) => AiluResult::ok(value),
        Err(error) => AiluResult::err(AILU_ERR_INPUT, error),
    }
}

fn into_c_string(value: String) -> *mut c_char {
    let nul_safe = value.replace('\0', "\\0");
    CString::new(nul_safe)
        .expect("internal NULs were escaped before building CString")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn returns_version_string() {
        let ptr = ailu_engine_version();
        assert!(!ptr.is_null());
        let version = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        assert!(!version.is_empty());
        unsafe {
            ailu_string_free(ptr);
        }
    }

    #[test]
    fn validates_input_json_errors() {
        let input = CString::new("{").unwrap();
        let result = unsafe { ailu_validate_graph_json(input.as_ptr()) };

        assert_eq!(result.code, AILU_ERR_INPUT);
        assert!(result.value.is_null());
        assert!(!result.error.is_null());

        let error = unsafe { CStr::from_ptr(result.error) }.to_str().unwrap();
        assert!(error.contains("invalid graph JSON"));
        unsafe {
            ailu_result_free(result);
        }
    }

    #[test]
    fn rejects_null_input() {
        let result = unsafe { ailu_compile_graph_yaml_json(ptr::null()) };

        assert_eq!(result.code, AILU_ERR_NULL);
        assert!(result.value.is_null());
        assert!(!result.error.is_null());

        unsafe {
            ailu_result_free(result);
        }
    }

    #[test]
    fn resolves_model_with_optional_inputs() {
        let tier = CString::new("fast").unwrap();
        let available = CString::new("[\"mistral\"]").unwrap();
        let result =
            unsafe { ailu_resolve_model_json(tier.as_ptr(), available.as_ptr(), ptr::null()) };

        assert_eq!(result.code, AILU_OK);
        assert!(!result.value.is_null());
        let value = unsafe { CStr::from_ptr(result.value) }.to_str().unwrap();
        assert!(value.contains("\"provider\":\"mistral\""));
        assert!(value.contains("\"model\":\"mistral-small-latest\""));

        unsafe {
            ailu_result_free(result);
        }
    }

    #[test]
    fn exposes_catalogs() {
        let components = ailu_list_components_json();
        assert_eq!(components.code, AILU_OK);
        let components_json = unsafe { CStr::from_ptr(components.value) }
            .to_str()
            .unwrap();
        assert!(components_json.contains("promptBuilder"));
        unsafe {
            ailu_result_free(components);
        }

        let prebuilt = ailu_list_prebuilt_json();
        assert_eq!(prebuilt.code, AILU_OK);
        let prebuilt_json = unsafe { CStr::from_ptr(prebuilt.value) }.to_str().unwrap();
        assert!(prebuilt_json.contains("summarizer"));
        unsafe {
            ailu_result_free(prebuilt);
        }
    }

    #[test]
    fn runs_component() {
        let kind = CString::new("promptBuilder").unwrap();
        let params =
            CString::new("{\"template\":\"Hello {{name}}!\",\"into\":\"prompt\"}").unwrap();
        let channels = CString::new("{\"name\":\"Ada\"}").unwrap();
        let result =
            unsafe { ailu_run_component_json(kind.as_ptr(), params.as_ptr(), channels.as_ptr()) };

        assert_eq!(result.code, AILU_OK);
        let output = unsafe { CStr::from_ptr(result.value) }.to_str().unwrap();
        assert_eq!(output, "{\"prompt\":\"Hello Ada!\"}");

        unsafe {
            ailu_result_free(result);
        }
    }

    struct CallbackCounters {
        nodes: AtomicUsize,
        conditions: AtomicUsize,
        events: AtomicUsize,
    }

    unsafe extern "C" fn node_callback(
        payload_json: *const c_char,
        user_data: *mut c_void,
        value: *mut *const c_char,
        error: *mut *const c_char,
    ) -> c_int {
        let counters = unsafe { &*(user_data as *const CallbackCounters) };
        counters.nodes.fetch_add(1, Ordering::SeqCst);
        let payload = unsafe { CStr::from_ptr(payload_json) }.to_str().unwrap();
        let output = if payload.contains("\"nodeId\":\"finish\"") {
            c"{\"done\":true}".as_ptr()
        } else {
            c"{\"seen\":\"start\"}".as_ptr()
        };
        unsafe {
            *value = output;
            *error = ptr::null();
        }
        AILU_OK
    }

    unsafe extern "C" fn condition_callback(
        _payload_json: *const c_char,
        user_data: *mut c_void,
        value: *mut *const c_char,
        error: *mut *const c_char,
    ) -> c_int {
        let counters = unsafe { &*(user_data as *const CallbackCounters) };
        counters.conditions.fetch_add(1, Ordering::SeqCst);
        unsafe {
            *value = c"true".as_ptr();
            *error = ptr::null();
        }
        AILU_OK
    }

    unsafe extern "C" fn event_callback(_payload_json: *const c_char, user_data: *mut c_void) {
        let counters = unsafe { &*(user_data as *const CallbackCounters) };
        counters.events.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn runs_callback_graph_through_c_abi() {
        let spec = CString::new(
            r#"{
              "graph": {
                "id": "callback-graph",
                "version": "1.0.0",
                "name": "Callback graph",
                "entryNodeId": "start",
                "channels": {
                  "seen": { "type": "string", "reducer": "replace" },
                  "done": { "type": "boolean", "reducer": "replace" }
                },
                "nodes": [
                  { "id": "start", "type": "action", "label": "Start" },
                  { "id": "finish", "type": "action", "label": "Finish" }
                ],
                "edges": [
                  { "id": "e1", "from": "start", "to": "finish", "type": "conditional", "condition": "go" }
                ]
              },
              "runId": "run-c",
              "jsNodeIds": ["start", "finish"]
            }"#,
        )
        .unwrap();
        let counters = CallbackCounters {
            nodes: AtomicUsize::new(0),
            conditions: AtomicUsize::new(0),
            events: AtomicUsize::new(0),
        };
        let callbacks = AiluCallbacks {
            user_data: (&counters as *const CallbackCounters).cast_mut().cast(),
            on_node: Some(node_callback),
            on_condition: Some(condition_callback),
            on_event: Some(event_callback),
        };

        let result = unsafe { ailu_engine_run_json(spec.as_ptr(), callbacks) };

        assert_eq!(result.code, AILU_OK);
        assert!(!result.value.is_null());
        let output = unsafe { CStr::from_ptr(result.value) }.to_str().unwrap();
        let json: serde_json::Value = serde_json::from_str(output).unwrap();
        assert_eq!(json["status"], "completed");
        assert_eq!(json["state"]["channels"]["seen"], "start");
        assert_eq!(json["state"]["channels"]["done"], true);
        assert_eq!(counters.nodes.load(Ordering::SeqCst), 2);
        assert_eq!(counters.conditions.load(Ordering::SeqCst), 1);
        assert!(counters.events.load(Ordering::SeqCst) >= 3);

        unsafe {
            ailu_result_free(result);
        }
    }
}
