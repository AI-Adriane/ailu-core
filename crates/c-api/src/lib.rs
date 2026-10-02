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
/// Polled by the run loop at every node boundary (ADR 0044): a non-zero return stops the run
/// there with status `"cancelled"`, after its last checkpoint.
pub type AiluCancelCallback = Option<unsafe extern "C" fn(user_data: *mut c_void) -> c_int>;

/// The callbacks of the original entry points, passed by value. Its layout is frozen: an SDK
/// built against an earlier release lays it out itself. Cancellation needs [`AiluCallbacksV2`].
#[repr(C)]
pub struct AiluCallbacks {
    pub user_data: *mut c_void,
    pub on_node: AiluStringCallback,
    pub on_condition: AiluStringCallback,
    pub on_event: AiluEventCallback,
}

/// The callbacks of the `_v2` entry points (ADR 0045 D2.4), passed by pointer: [`AiluCallbacks`]
/// plus `is_cancelled`. `struct_size` must be `sizeof(AiluCallbacksV2)`; a later release appends
/// fields after `is_cancelled` and reads them only from callers that sent a larger size, so a
/// caller built against this release keeps working.
#[repr(C)]
pub struct AiluCallbacksV2 {
    pub struct_size: usize,
    pub user_data: *mut c_void,
    pub on_node: AiluStringCallback,
    pub on_condition: AiluStringCallback,
    pub on_event: AiluEventCallback,
    /// `NULL` reads as "never cancelled".
    pub is_cancelled: AiluCancelCallback,
}

#[derive(Clone, Copy)]
struct CCallbacks {
    user_data: usize,
    on_node: AiluStringCallback,
    on_condition: AiluStringCallback,
    on_event: AiluEventCallback,
    is_cancelled: AiluCancelCallback,
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
            is_cancelled: None,
        }
    }
}

impl CCallbacks {
    /// Read the callbacks of a `_v2` entry point, refusing a null pointer or a `struct_size`
    /// smaller than this release's [`AiluCallbacksV2`].
    ///
    /// # Safety
    ///
    /// `callbacks` must be null or point to at least `struct_size` readable bytes, starting with
    /// an [`AiluCallbacksV2`].
    unsafe fn from_v2(callbacks: *const AiluCallbacksV2) -> Result<Self, AiluResult> {
        if callbacks.is_null() {
            return Err(AiluResult::err(
                AILU_ERR_NULL,
                "callbacks pointer must not be null",
            ));
        }
        // `struct_size` is the first field: always readable, whatever the caller's version.
        let size = unsafe { ptr::addr_of!((*callbacks).struct_size).read() };
        if size < std::mem::size_of::<AiluCallbacksV2>() {
            return Err(AiluResult::err(
                AILU_ERR_INPUT,
                format!(
                    "callbacks struct_size is {size}, expected at least sizeof(AiluCallbacksV2) = {}",
                    std::mem::size_of::<AiluCallbacksV2>()
                ),
            ));
        }
        let callbacks = unsafe { &*callbacks };
        Ok(Self {
            user_data: callbacks.user_data as usize,
            on_node: callbacks.on_node,
            on_condition: callbacks.on_condition,
            on_event: callbacks.on_event,
            is_cancelled: callbacks.is_cancelled,
        })
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

    /// ADR 0044 through the `_v2` entry points: a non-zero answer stops the run at this node
    /// boundary. No callback (or a `_v1` entry point) never cancels.
    fn is_cancelled(&self) -> bool {
        self.is_cancelled
            .is_some_and(|callback| unsafe { callback(self.user_data as *mut c_void) } != 0)
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

/// Build the `EngineSpec` of a catalog graph (ADR 0045 D3.2): the engine reads the `component` /
/// `agent` / `mapAgents` carriers of the graph's and its subgraphs' nodes. `input_json` is
/// `{ graph, subgraphs?, hostNodes?, hostTools?, providerKeys?, fsPolicy?, skills? }`; the value is
/// `{ spec, warnings }`, or `{ error: { kind, message, nodeId?, reason? } }` when a host node
/// binding or a carrier cannot be used. `AILU_ERR_INPUT` only for input that is not JSON.
///
/// # Safety
///
/// `input_json` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_spec_from_catalog_json(input_json: *const c_char) -> AiluResult {
    unsafe {
        with_c_str(input_json, |raw| {
            ailu_runtime_bridge::catalog::catalog_spec_json(raw)
                .map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
}

/// What a governed catalog run files with its approval store (ADR 0045 D3.1). `input_json` is
/// `{ graph, subgraphs?, state, previousState? }` (`previousState` for a resume); the value is
/// `{ clearApprovalIds, requests: [{ runId, nodeId, requestedBy, subject }] }`.
///
/// # Safety
///
/// `input_json` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_catalog_approval_plan_json(input_json: *const c_char) -> AiluResult {
    unsafe {
        with_c_str(input_json, |raw| {
            ailu_runtime_bridge::catalog_approvals::filing_plan_json(raw)
                .map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
}

/// The stashed approval ids a resume of a catalog run must read back from its store (ADR 0045
/// D3.1): the suspended `GraphState` in, a JSON array of ids out.
///
/// # Safety
///
/// `state_json` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_catalog_approvals_to_check_json(
    state_json: *const c_char,
) -> AiluResult {
    unsafe {
        with_c_str(state_json, |raw| {
            ailu_runtime_bridge::catalog_approvals::approvals_to_check_json(raw)
                .map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
}

/// Why a resume of a catalog run may not go on (ADR 0045 D3.1). `input_json` is `{ graph,
/// subgraphs?, state, approvedTools?, approvals: { <id>: record | null } }`; the value is a JSON
/// array of problems, empty when the resume may go on.
///
/// # Safety
///
/// `input_json` must be a valid, null-terminated UTF-8 C string pointer.
#[no_mangle]
pub unsafe extern "C" fn ailu_catalog_resume_problems_json(
    input_json: *const c_char,
) -> AiluResult {
    unsafe {
        with_c_str(input_json, |raw| {
            ailu_runtime_bridge::catalog_approvals::resume_problems_json(raw)
                .map_err(|error| (AILU_ERR_INPUT, error))
        })
    }
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
    let entry = match unsafe { signal_entry(signal_name, payload_json) } {
        Ok(entry) => entry,
        Err(result) => return result,
    };
    unsafe { run_engine_entry(spec_json, callbacks, entry) }
}

/// The entry delivering signal `signal_name` with its JSON payload.
unsafe fn signal_entry(
    signal_name: *const c_char,
    payload_json: *const c_char,
) -> Result<Entry, AiluResult> {
    let name = unsafe { read_required_c_str(signal_name) }?.to_owned();
    let payload = serde_json::from_str::<Value>(unsafe { read_required_c_str(payload_json) }?)
        .map_err(|error| {
            AiluResult::err(
                AILU_ERR_INPUT,
                format!("invalid signal payload JSON: {error}"),
            )
        })?;
    Ok(Entry::Signal { name, payload })
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

/// [`ailu_engine_run_json`] with cancellation (ADR 0045 D2.4): `callbacks->is_cancelled` is
/// polled at every node boundary, and a non-zero answer stops the run there with status
/// `"cancelled"`, its last checkpoint intact.
///
/// # Safety
///
/// `spec_json` must be a valid, null-terminated UTF-8 C string pointer. `callbacks` must point to
/// an `AiluCallbacksV2` whose `struct_size` is set, and every function pointer in it, when present,
/// must be valid for the full duration of this call.
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_run_json_v2(
    spec_json: *const c_char,
    callbacks: *const AiluCallbacksV2,
) -> AiluResult {
    unsafe { run_engine_entry_v2(spec_json, callbacks, Entry::Start) }
}

/// [`ailu_engine_resume_json`] with cancellation (see [`ailu_engine_run_json_v2`]).
///
/// # Safety
///
/// Same requirements as [`ailu_engine_run_json_v2`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_resume_json_v2(
    spec_json: *const c_char,
    callbacks: *const AiluCallbacksV2,
) -> AiluResult {
    unsafe { run_engine_entry_v2(spec_json, callbacks, Entry::Resume) }
}

/// [`ailu_engine_approve_and_resume_json`] with cancellation (see [`ailu_engine_run_json_v2`]).
///
/// # Safety
///
/// Same requirements as [`ailu_engine_run_json_v2`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_approve_and_resume_json_v2(
    spec_json: *const c_char,
    callbacks: *const AiluCallbacksV2,
) -> AiluResult {
    unsafe { run_engine_entry_v2(spec_json, callbacks, Entry::Approve) }
}

/// [`ailu_engine_signal_json`] with cancellation (see [`ailu_engine_run_json_v2`]).
///
/// # Safety
///
/// All string pointers must be valid, null-terminated UTF-8 C strings; `callbacks` as for
/// [`ailu_engine_run_json_v2`].
#[no_mangle]
pub unsafe extern "C" fn ailu_engine_signal_json_v2(
    spec_json: *const c_char,
    signal_name: *const c_char,
    payload_json: *const c_char,
    callbacks: *const AiluCallbacksV2,
) -> AiluResult {
    let entry = match unsafe { signal_entry(signal_name, payload_json) } {
        Ok(entry) => entry,
        Err(result) => return result,
    };
    unsafe { run_engine_entry_v2(spec_json, callbacks, entry) }
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
    unsafe { run_with_callbacks(spec_json, CCallbacks::from(callbacks), entry) }
}

unsafe fn run_engine_entry_v2(
    spec_json: *const c_char,
    callbacks: *const AiluCallbacksV2,
    entry: Entry,
) -> AiluResult {
    match unsafe { CCallbacks::from_v2(callbacks) } {
        Ok(callbacks) => unsafe { run_with_callbacks(spec_json, callbacks, entry) },
        Err(result) => result,
    }
}

unsafe fn run_with_callbacks(
    spec_json: *const c_char,
    callbacks: CCallbacks,
    entry: Entry,
) -> AiluResult {
    let spec = match unsafe { read_required_c_str(spec_json) } {
        Ok(value) => value.to_owned(),
        Err(result) => return result,
    };
    let callbacks: SharedCallbacks = std::sync::Arc::new(callbacks);
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

    /// The host state of the `_v2` tests: callback counters, and the answer to `is_cancelled`.
    struct CancelHost {
        counters: CallbackCounters,
        cancel: bool,
        polls: AtomicUsize,
    }

    unsafe extern "C" fn v2_node_callback(
        payload_json: *const c_char,
        user_data: *mut c_void,
        value: *mut *const c_char,
        error: *mut *const c_char,
    ) -> c_int {
        let host = unsafe { &*(user_data as *const CancelHost) };
        unsafe {
            node_callback(
                payload_json,
                (&host.counters as *const CallbackCounters)
                    .cast_mut()
                    .cast(),
                value,
                error,
            )
        }
    }

    unsafe extern "C" fn cancel_callback(user_data: *mut c_void) -> c_int {
        let host = unsafe { &*(user_data as *const CancelHost) };
        host.polls.fetch_add(1, Ordering::SeqCst);
        c_int::from(host.cancel)
    }

    fn two_step_spec() -> CString {
        CString::new(
            r#"{
              "graph": {
                "id": "two-steps", "version": "1.0.0", "name": "Two steps", "entryNodeId": "start",
                "channels": {
                  "seen": { "type": "string", "reducer": "replace" },
                  "done": { "type": "boolean", "reducer": "replace" }
                },
                "nodes": [
                  { "id": "start", "type": "action", "label": "Start" },
                  { "id": "finish", "type": "action", "label": "Finish" }
                ],
                "edges": [{ "id": "e1", "from": "start", "to": "finish", "type": "default" }]
              },
              "runId": "run-v2",
              "hostNodeIds": ["start", "finish"]
            }"#,
        )
        .unwrap()
    }

    fn host(cancel: bool) -> CancelHost {
        CancelHost {
            counters: CallbackCounters {
                nodes: AtomicUsize::new(0),
                conditions: AtomicUsize::new(0),
                events: AtomicUsize::new(0),
            },
            cancel,
            polls: AtomicUsize::new(0),
        }
    }

    fn v2_callbacks(host: &CancelHost) -> AiluCallbacksV2 {
        AiluCallbacksV2 {
            struct_size: std::mem::size_of::<AiluCallbacksV2>(),
            user_data: (host as *const CancelHost).cast_mut().cast(),
            on_node: Some(v2_node_callback),
            on_condition: None,
            on_event: None,
            is_cancelled: Some(cancel_callback),
        }
    }

    fn outcome(result: AiluResult) -> serde_json::Value {
        assert_eq!(result.code, AILU_OK);
        let json = serde_json::from_str(unsafe { CStr::from_ptr(result.value) }.to_str().unwrap())
            .unwrap();
        unsafe { ailu_result_free(result) };
        json
    }

    #[test]
    fn a_v2_run_stops_at_the_first_boundary_when_cancelled() {
        let host = host(true);
        let spec = two_step_spec();
        let result = unsafe { ailu_engine_run_json_v2(spec.as_ptr(), &v2_callbacks(&host)) };
        let json = outcome(result);
        assert_eq!(json["status"], "cancelled");
        assert_eq!(host.counters.nodes.load(Ordering::SeqCst), 0);
        assert!(host.polls.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn a_v2_run_that_is_never_cancelled_runs_to_the_end_polling_each_boundary() {
        let host = host(false);
        let spec = two_step_spec();
        let result = unsafe { ailu_engine_run_json_v2(spec.as_ptr(), &v2_callbacks(&host)) };
        let json = outcome(result);
        assert_eq!(json["status"], "completed");
        assert_eq!(json["state"]["channels"]["done"], true);
        assert_eq!(host.counters.nodes.load(Ordering::SeqCst), 2);
        assert!(host.polls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn a_v2_run_without_a_cancel_callback_is_never_cancelled() {
        let host = host(true);
        let spec = two_step_spec();
        let callbacks = AiluCallbacksV2 {
            is_cancelled: None,
            ..v2_callbacks(&host)
        };
        let json = outcome(unsafe { ailu_engine_run_json_v2(spec.as_ptr(), &callbacks) });
        assert_eq!(json["status"], "completed");
        assert_eq!(host.polls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_v2_resume_is_cancellable_too() {
        let host = host(true);
        let spec = CString::new(
            r#"{
              "graph": {
                "id": "gated", "version": "1.0.0", "name": "Gated", "entryNodeId": "gate",
                "channels": {}, "edges": [{ "id": "e1", "from": "gate", "to": "finish", "type": "default" }],
                "nodes": [
                  { "id": "gate", "type": "human-gate", "label": "Gate" },
                  { "id": "finish", "type": "action", "label": "Finish" }
                ]
              },
              "hostNodeIds": ["finish"],
              "state": {
                "runId": "run-gated", "graphId": "gated", "currentNodeId": "gate",
                "status": "suspended", "channels": {}, "version": 1,
                "createdAt": "0", "updatedAt": "0"
              }
            }"#,
        )
        .unwrap();
        let json =
            outcome(unsafe { ailu_engine_resume_json_v2(spec.as_ptr(), &v2_callbacks(&host)) });
        assert_eq!(json["status"], "cancelled");
        assert_eq!(host.counters.nodes.load(Ordering::SeqCst), 0);
    }

    fn ok_json(result: AiluResult) -> serde_json::Value {
        assert_eq!(result.code, AILU_OK);
        let json = serde_json::from_str(unsafe { CStr::from_ptr(result.value) }.to_str().unwrap())
            .unwrap();
        unsafe { ailu_result_free(result) };
        json
    }

    #[test]
    fn builds_a_catalog_spec_and_decides_its_approvals() {
        let graph = serde_json::json!({
            "id": "g", "version": "1", "name": "g", "channels": {}, "entryNodeId": "review",
            "nodes": [
                { "id": "review", "type": "human-gate", "label": "review" },
                { "id": "send", "type": "action", "label": "send" },
                { "id": "assistant", "type": "agent", "label": "assistant",
                  "metadata": { "agent": { "toolNames": ["refund"] } } }
            ],
            "edges": []
        });
        let input =
            CString::new(serde_json::json!({ "graph": graph, "hostNodes": ["send"] }).to_string())
                .unwrap();
        let built = ok_json(unsafe { ailu_spec_from_catalog_json(input.as_ptr()) });
        assert_eq!(built["spec"]["hostNodeIds"], serde_json::json!(["send"]));
        assert_eq!(
            built["spec"]["agents"]["assistant"]["provider"],
            "anthropic"
        );

        let state = serde_json::json!({
            "runId": "run-1", "graphId": "g", "currentNodeId": "review", "status": "suspended",
            "channels": {}, "version": 1, "createdAt": "0", "updatedAt": "0"
        });
        let plan_input =
            CString::new(serde_json::json!({ "graph": graph, "state": state }).to_string())
                .unwrap();
        let plan = ok_json(unsafe { ailu_catalog_approval_plan_json(plan_input.as_ptr()) });
        assert_eq!(plan["requests"][0]["subject"]["description"], "gate:review");

        let state_json = CString::new(state.to_string()).unwrap();
        let to_check =
            ok_json(unsafe { ailu_catalog_approvals_to_check_json(state_json.as_ptr()) });
        assert_eq!(to_check, serde_json::json!([]));

        let problems = ok_json(unsafe { ailu_catalog_resume_problems_json(plan_input.as_ptr()) });
        assert_eq!(
            problems.as_array().map(Vec::len),
            Some(1),
            "never recorded: {problems}"
        );

        let bad = CString::new("{").unwrap();
        for result in unsafe {
            [
                ailu_spec_from_catalog_json(bad.as_ptr()),
                ailu_catalog_approval_plan_json(bad.as_ptr()),
                ailu_catalog_approvals_to_check_json(bad.as_ptr()),
                ailu_catalog_resume_problems_json(bad.as_ptr()),
            ]
        } {
            assert_eq!(result.code, AILU_ERR_INPUT);
            unsafe { ailu_result_free(result) };
        }
    }

    #[test]
    fn callbacks_v2_has_the_layout_of_the_header() {
        // `include/ailu.h`: size_t, then five pointers — the original four fields in their order,
        // then `is_cancelled`.
        let word = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<AiluCallbacksV2>(), 6 * word);
        assert_eq!(std::mem::offset_of!(AiluCallbacksV2, user_data), word);
        assert_eq!(
            std::mem::offset_of!(AiluCallbacksV2, is_cancelled),
            5 * word
        );
        assert_eq!(std::mem::size_of::<AiluCallbacks>(), 4 * word);
    }

    #[test]
    fn v2_entry_points_refuse_a_null_or_short_callbacks_struct() {
        let spec = two_step_spec();
        let result = unsafe { ailu_engine_run_json_v2(spec.as_ptr(), ptr::null()) };
        assert_eq!(result.code, AILU_ERR_NULL);
        unsafe { ailu_result_free(result) };

        let host = host(false);
        let short = AiluCallbacksV2 {
            struct_size: std::mem::size_of::<AiluCallbacks>(),
            ..v2_callbacks(&host)
        };
        let result = unsafe { ailu_engine_approve_and_resume_json_v2(spec.as_ptr(), &short) };
        assert_eq!(result.code, AILU_ERR_INPUT);
        let error = unsafe { CStr::from_ptr(result.error) }.to_str().unwrap();
        assert!(error.contains("struct_size"), "{error}");
        unsafe { ailu_result_free(result) };
        assert_eq!(host.counters.nodes.load(Ordering::SeqCst), 0);

        let name = CString::new("paid").unwrap();
        let payload = CString::new("not json").unwrap();
        let result = unsafe {
            ailu_engine_signal_json_v2(
                spec.as_ptr(),
                name.as_ptr(),
                payload.as_ptr(),
                &v2_callbacks(&host),
            )
        };
        assert_eq!(result.code, AILU_ERR_INPUT);
        unsafe { ailu_result_free(result) };
    }
}
