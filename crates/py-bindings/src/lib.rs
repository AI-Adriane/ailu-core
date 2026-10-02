//! Python bindings (pyo3) over the Rust engine. JSON in / JSON out, mirroring the
//! napi surface, so the Python SDK calls the same Rust core as the TypeScript SDK
//! without crossing complex type boundaries. Proves the multi-language-SDK strategy:
//! one Rust engine, thin per-language SDKs.
//!
//! With the `extension-module` feature, pyo3 does not link libpython at build time,
//! so `cargo build` succeeds without python-dev linkage.
//!
//! The model policy, the component/prebuilt catalogs, and the one-shot component and
//! prebuilt runs execute FULLY on Rust, with no host callbacks. The graph runner
//! (`engine_run`, `engine_resume`, `engine_approve_and_resume`, `engine_signal`,
//! `engine_replay` — ADR 0045 D2.2) is the callback-capable one the TypeScript and C ABI
//! SDKs drive: it takes Python callables for host nodes and tools, conditions, events and
//! cancellation (see [`runner`]). It releases the GIL for the run and takes it back in
//! each callback, all on the calling thread.
//!
//! Layering: all logic lives in [`core`] as plain `fn(..) -> Result<String, String>`
//! (JSON in / JSON out, no pyo3 types). The [`pyo3` layer](self) is a `#[cfg(not(test))]`
//! set of thin `#[pyfunction]` wrappers that map a `core` error string onto a
//! `PyValueError`. Gating the pyo3 entry points out of `cfg(test)` keeps the
//! `cargo test` harness binary free of CPython symbols, so it runs without a Python
//! interpreter or libpython linkage while still exercising the real `core` logic.

#![forbid(unsafe_code)]
#![deny(clippy::all)]
// pyo3's `#[pyfunction]` expansion emits an error-conversion call at each wrapped
// function's signature that round-trips an already-`PyErr` value through `From`.
// clippy reports that macro-generated code as a useless conversion; the span sits
// outside any function body so a local `#[allow]` cannot reach it. Relax only this
// one lint (every other `clippy::all` lint stays denied).
#![allow(clippy::useless_conversion)]

pub mod core;
pub mod model;
pub mod runner;

// ---------------------------------------------------------------------------
// pyo3 layer — thin `#[pyfunction]` wrappers over `core`. Gated out of test builds
// so the test harness never links the CPython symbols these wrappers reference.
// ---------------------------------------------------------------------------
#[cfg(not(test))]
mod py {
    use crate::core;
    use crate::model;
    use crate::runner::{self, CancelFn, ConditionFn, EventFn, HostFns, NodeFn};
    use ailu_runtime_bridge::Entry;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    /// Map a `core` error string onto a `PyValueError`.
    fn to_py(result: Result<String, String>) -> PyResult<String> {
        result.map_err(PyValueError::new_err)
    }

    /// Wrap the Python callables of a run into the runner's host functions. Each takes the GIL
    /// for its call. An exception in `on_node` or `on_condition` fails what called it, with the
    /// exception as the error; one in `on_event` or `is_cancelled` cannot propagate, so it is
    /// reported the way Python reports such errors (`sys.unraisablehook`), and a failing
    /// cancellation check reads as "not cancelled".
    fn host_fns(
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
        is_cancelled: Option<Py<PyAny>>,
    ) -> HostFns {
        HostFns {
            on_node: on_node.map(|callable| -> NodeFn {
                Box::new(move |payload| {
                    Python::attach(|py| {
                        callable
                            .call1(py, (payload,))
                            .and_then(|result| result.bind(py).extract::<String>())
                            .map_err(|error| error.to_string())
                    })
                })
            }),
            on_condition: on_condition.map(|callable| -> ConditionFn {
                Box::new(move |payload| {
                    Python::attach(|py| {
                        callable
                            .call1(py, (payload,))
                            .and_then(|result| result.bind(py).is_truthy())
                            .map_err(|error| error.to_string())
                    })
                })
            }),
            on_event: on_event.map(|callable| -> EventFn {
                Box::new(move |payload| {
                    Python::attach(|py| {
                        if let Err(error) = callable.call1(py, (payload,)) {
                            error.write_unraisable(py, None);
                        }
                    })
                })
            }),
            is_cancelled: is_cancelled.map(|callable| -> CancelFn {
                Box::new(move || {
                    Python::attach(|py| {
                        match callable
                            .call0(py)
                            .and_then(|result| result.bind(py).is_truthy())
                        {
                            Ok(cancelled) => cancelled,
                            Err(error) => {
                                error.write_unraisable(py, None);
                                false
                            }
                        }
                    })
                })
            }),
        }
    }

    /// Run `entry` over `spec_json` with the GIL released; callbacks take it back.
    fn drive(py: Python<'_>, spec_json: String, host: HostFns, entry: Entry) -> PyResult<String> {
        to_py(py.detach(move || runner::run(&spec_json, host, entry)))
    }

    /// Start a run of an `EngineSpec` (JSON string). Returns the `RunOutcome` JSON.
    #[pyfunction]
    #[pyo3(signature = (spec_json, on_node = None, on_condition = None, on_event = None, is_cancelled = None))]
    fn engine_run(
        py: Python<'_>,
        spec_json: String,
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
        is_cancelled: Option<Py<PyAny>>,
    ) -> PyResult<String> {
        let host = host_fns(on_node, on_condition, on_event, is_cancelled);
        drive(py, spec_json, host, Entry::Start)
    }

    /// Resume a suspended run from the spec's `state` (past a gate, a timer, an interrupt).
    #[pyfunction]
    #[pyo3(signature = (spec_json, on_node = None, on_condition = None, on_event = None, is_cancelled = None))]
    fn engine_resume(
        py: Python<'_>,
        spec_json: String,
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
        is_cancelled: Option<Py<PyAny>>,
    ) -> PyResult<String> {
        let host = host_fns(on_node, on_condition, on_event, is_cancelled);
        drive(py, spec_json, host, Entry::Resume)
    }

    /// Grant the spec's `approvedTools` (the engine re-checks that no one approved their own
    /// request), then resume.
    #[pyfunction]
    #[pyo3(signature = (spec_json, on_node = None, on_condition = None, on_event = None, is_cancelled = None))]
    fn engine_approve_and_resume(
        py: Python<'_>,
        spec_json: String,
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
        is_cancelled: Option<Py<PyAny>>,
    ) -> PyResult<String> {
        let host = host_fns(on_node, on_condition, on_event, is_cancelled);
        drive(py, spec_json, host, Entry::Approve)
    }

    /// Deliver signal `name` (payload as JSON) to a run waiting on it, then resume.
    #[pyfunction]
    #[pyo3(signature = (spec_json, name, payload_json, on_node = None, on_condition = None, on_event = None, is_cancelled = None))]
    #[allow(clippy::too_many_arguments)]
    fn engine_signal(
        py: Python<'_>,
        spec_json: String,
        name: String,
        payload_json: String,
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
        is_cancelled: Option<Py<PyAny>>,
    ) -> PyResult<String> {
        let entry = runner::signal_entry(&name, &payload_json).map_err(PyValueError::new_err)?;
        let host = host_fns(on_node, on_condition, on_event, is_cancelled);
        drive(py, spec_json, host, entry)
    }

    /// Replay a recorded run from `checkpoint_id` with the spec's `replayJournal` (ADR 0038):
    /// recorded model outputs, tool results and host-node results are served, never re-run.
    #[pyfunction]
    #[pyo3(signature = (spec_json, checkpoint_id, on_node = None, on_condition = None, on_event = None))]
    fn engine_replay(
        py: Python<'_>,
        spec_json: String,
        checkpoint_id: String,
        on_node: Option<Py<PyAny>>,
        on_condition: Option<Py<PyAny>>,
        on_event: Option<Py<PyAny>>,
    ) -> PyResult<String> {
        let host = host_fns(on_node, on_condition, on_event, None);
        drive(py, spec_json, host, Entry::Replay { checkpoint_id })
    }

    /// Build the `EngineSpec` of a catalog graph (ADR 0045 D3.2) from `{ graph, subgraphs?,
    /// hostNodes?, hostTools?, providerKeys?, fsPolicy?, skills? }` (JSON). Returns
    /// `{ spec, warnings }` or `{ error: { kind, message, nodeId?, reason? } }` as JSON; raises
    /// `ValueError` only on malformed input JSON.
    #[pyfunction]
    fn engine_spec_from_catalog(input_json: String) -> PyResult<String> {
        to_py(ailu_runtime_bridge::catalog::catalog_spec_json(&input_json))
    }

    /// What a catalog run's host files in its approval store (ADR 0045 D3.1). See
    /// `ailu_runtime_bridge::catalog_approvals::filing_plan`.
    #[pyfunction]
    fn engine_catalog_approval_plan(input_json: String) -> PyResult<String> {
        to_py(ailu_runtime_bridge::catalog_approvals::filing_plan_json(
            &input_json,
        ))
    }

    /// The stashed approval ids a resume of a catalog run must read back (ADR 0045 D3.1).
    #[pyfunction]
    fn engine_catalog_approvals_to_check(state_json: String) -> PyResult<String> {
        to_py(ailu_runtime_bridge::catalog_approvals::approvals_to_check_json(&state_json))
    }

    /// Why a resume of a catalog run may not go on (ADR 0045 D3.1): a JSON array of problems.
    #[pyfunction]
    fn engine_catalog_resume_problems(input_json: String) -> PyResult<String> {
        to_py(ailu_runtime_bridge::catalog_approvals::resume_problems_json(&input_json))
    }

    /// Compare an attested chain's ordered decisions with a replay's (ADR 0045 D3.4). Returns
    /// `{ ok, attested, replayed, mismatches }` as JSON.
    #[pyfunction]
    fn engine_verify_replay_decisions(
        attested_json: String,
        replayed_json: String,
    ) -> PyResult<String> {
        to_py(
            ailu_runtime_bridge::run_insight::verify_replay_decisions_json(
                &attested_json,
                &replayed_json,
            ),
        )
    }

    /// Explain a run from its `GraphState` and, optionally, its run events (ADR 0045 D3.4), as
    /// JSON.
    #[pyfunction]
    #[pyo3(signature = (state_json, events_json = None))]
    fn engine_explain_run(state_json: String, events_json: Option<String>) -> PyResult<String> {
        to_py(ailu_runtime_bridge::run_insight::explain_run_json(
            &state_json,
            events_json.as_deref(),
        ))
    }

    /// One-shot model call (a serialized `LlmRequest` and a provider keys map, as JSON); returns
    /// the `LlmResponse` JSON. The GIL is released for the call.
    #[pyfunction]
    fn llm_complete(
        py: Python<'_>,
        request_json: String,
        provider_keys_json: String,
    ) -> PyResult<String> {
        to_py(py.detach(move || model::llm_complete(&request_json, &provider_keys_json)))
    }

    /// Version of the bound Rust engine.
    #[pyfunction]
    fn engine_version() -> String {
        core::engine_version()
    }

    /// Validate a graph definition (JSON string). Returns a JSON array of validation
    /// errors — empty (`[]`) when the graph is structurally sound. Raises `ValueError`
    /// on malformed JSON.
    #[pyfunction]
    fn validate_graph_json(definition_json: String) -> PyResult<String> {
        to_py(core::validate_graph_json(&definition_json))
    }

    /// Compile graph DSL YAML into a validated `GraphDefinition` (JSON string).
    /// Raises `ValueError` on parse, DSL, or structural validation failure.
    #[pyfunction]
    fn compile_graph_yaml(yaml: String) -> PyResult<String> {
        to_py(core::compile_graph_yaml(&yaml))
    }

    /// Resolve a capability tier to a concrete `{ provider, model, recommended }`
    /// (the `ModelChoice` JSON). See [`core::resolve_model`].
    #[pyfunction]
    #[pyo3(signature = (tier, available_json = None, override_json = None))]
    fn resolve_model(
        tier: String,
        available_json: Option<String>,
        override_json: Option<String>,
    ) -> PyResult<String> {
        to_py(core::resolve_model(
            &tier,
            available_json.as_deref(),
            override_json.as_deref(),
        ))
    }

    /// The providers usable in the current process env (a JSON array of provider
    /// strings).
    #[pyfunction]
    fn available_providers() -> PyResult<String> {
        to_py(core::available_providers())
    }

    /// The component kinds the registry knows how to build (JSON array of strings).
    #[pyfunction]
    fn list_components() -> PyResult<String> {
        to_py(core::list_components())
    }

    /// Every prebuilt micro-agent definition (JSON array, camelCase).
    #[pyfunction]
    fn list_prebuilt() -> PyResult<String> {
        to_py(core::list_prebuilt())
    }

    /// Run a single component handler, fully on Rust. See [`core::run_component`].
    #[pyfunction]
    fn run_component(kind: String, params_json: String, channels_json: String) -> PyResult<String> {
        to_py(core::run_component(&kind, &params_json, &channels_json))
    }

    /// Run a prebuilt micro-agent, fully on Rust. See [`core::run_prebuilt`].
    #[pyfunction]
    #[pyo3(signature = (name, input_json, options_json = None))]
    fn run_prebuilt(
        name: String,
        input_json: String,
        options_json: Option<String>,
    ) -> PyResult<String> {
        to_py(core::run_prebuilt(
            &name,
            &input_json,
            options_json.as_deref(),
        ))
    }

    /// The native `ailu` extension module. The function name MUST match the
    /// `[lib] name` ("ailu") so the built artifact imports under that name.
    #[pymodule]
    fn ailu(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_function(wrap_pyfunction!(engine_version, m)?)?;
        m.add_function(wrap_pyfunction!(validate_graph_json, m)?)?;
        m.add_function(wrap_pyfunction!(compile_graph_yaml, m)?)?;
        m.add_function(wrap_pyfunction!(resolve_model, m)?)?;
        m.add_function(wrap_pyfunction!(available_providers, m)?)?;
        m.add_function(wrap_pyfunction!(list_components, m)?)?;
        m.add_function(wrap_pyfunction!(list_prebuilt, m)?)?;
        m.add_function(wrap_pyfunction!(run_component, m)?)?;
        m.add_function(wrap_pyfunction!(run_prebuilt, m)?)?;
        m.add_function(wrap_pyfunction!(engine_run, m)?)?;
        m.add_function(wrap_pyfunction!(engine_resume, m)?)?;
        m.add_function(wrap_pyfunction!(engine_approve_and_resume, m)?)?;
        m.add_function(wrap_pyfunction!(engine_signal, m)?)?;
        m.add_function(wrap_pyfunction!(engine_replay, m)?)?;
        m.add_function(wrap_pyfunction!(engine_spec_from_catalog, m)?)?;
        m.add_function(wrap_pyfunction!(engine_catalog_approval_plan, m)?)?;
        m.add_function(wrap_pyfunction!(engine_catalog_approvals_to_check, m)?)?;
        m.add_function(wrap_pyfunction!(engine_catalog_resume_problems, m)?)?;
        m.add_function(wrap_pyfunction!(engine_verify_replay_decisions, m)?)?;
        m.add_function(wrap_pyfunction!(engine_explain_run, m)?)?;
        m.add_function(wrap_pyfunction!(llm_complete, m)?)?;
        Ok(())
    }
}
