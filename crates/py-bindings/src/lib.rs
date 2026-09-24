//! Python bindings (pyo3) over the Rust engine. JSON in / JSON out, mirroring the
//! napi surface, so the Python SDK calls the same Rust core as the TypeScript SDK
//! without crossing complex type boundaries. Proves the multi-language-SDK strategy:
//! one Rust engine, thin per-language SDKs.
//!
//! With the `extension-module` feature, pyo3 does not link libpython at build time,
//! so `cargo build` succeeds without python-dev linkage.
//!
//! Unlike the napi bridge (`crates/bindings`), these functions take NO host-language
//! callbacks: the model policy, the component/prebuilt catalogs, and the run paths
//! all execute FULLY on Rust. A run drives a current-thread tokio runtime with
//! `block_on` inside the (synchronous) `#[pyfunction]` — there are no Python
//! callbacks, so there is no deadlock risk.
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

// ---------------------------------------------------------------------------
// pyo3 layer — thin `#[pyfunction]` wrappers over `core`. Gated out of test builds
// so the test harness never links the CPython symbols these wrappers reference.
// ---------------------------------------------------------------------------
#[cfg(not(test))]
mod py {
    use crate::core;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    /// Map a `core` error string onto a `PyValueError`.
    fn to_py(result: Result<String, String>) -> PyResult<String> {
        result.map_err(PyValueError::new_err)
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
        Ok(())
    }
}
