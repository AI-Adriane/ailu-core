"""Ailu Python SDK.

A thin, Pythonic wrapper over the Ailu Rust engine, exposed through a native
pyo3 extension module. This SDK and the TypeScript SDK share ONE Rust engine —
the same graph validator and DSL compiler back both languages, so behaviour is
identical across the ecosystem (the multi-language-SDK strategy: one engine,
thin per-language SDKs).

The native extension (`ailu.ailu`, built from `crates/py-bindings`)
speaks JSON in / JSON out. This module hides that boundary, taking and returning
native Python `dict`/`list` values.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Optional

# The native extension is a submodule whose leaf import name ("ailu") matches
# the `PyInit_ailu` symbol emitted by the `#[pymodule] fn ailu` in Rust.
from . import ailu as _native  # type: ignore[attr-defined]

__all__ = [
    "validate_graph",
    "compile_graph_yaml",
    "engine_version",
    "available_providers",
    "resolve_model",
    "list_components",
    "list_prebuilt",
    "run_component",
    "run_prebuilt",
    "prebuilt",
    "GraphValidationError",
    "GraphCompileError",
    "RunError",
]


class GraphValidationError(ValueError):
    """Raised when a graph definition cannot even be parsed as JSON.

    Note: a *structurally invalid* graph does NOT raise — :func:`validate_graph`
    returns the list of validation errors instead. This is raised only when the
    input cannot be serialised/parsed at the JSON boundary.
    """


class GraphCompileError(ValueError):
    """Raised when DSL YAML fails to parse, compile, or validate."""


class RunError(ValueError):
    """Raised when a component or prebuilt-agent run fails.

    Covers an unknown component kind / agent name, invalid params or input, and a
    handler/runtime failure reported by the Rust engine.
    """


def engine_version() -> str:
    """Return the version string of the bound Rust engine."""
    return _native.engine_version()


def validate_graph(definition: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Validate a graph definition.

    Args:
        definition: A graph definition as a plain ``dict`` (the same shape as
            ``GraphDefinition`` JSON).

    Returns:
        A list of validation-error dicts, each with ``code`` (e.g.
        ``"INVALID_EDGE_REFERENCE"``), ``message``, and ``path``. An empty list
        means the graph is structurally sound.

    Raises:
        GraphValidationError: If the definition cannot be encoded/parsed as JSON.
    """
    try:
        payload = json.dumps(definition)
    except (TypeError, ValueError) as error:
        raise GraphValidationError(f"definition is not JSON-serialisable: {error}") from error
    try:
        result = _native.validate_graph_json(payload)
    except ValueError as error:
        raise GraphValidationError(str(error)) from error
    return json.loads(result)


def compile_graph_yaml(yaml: str) -> Dict[str, Any]:
    """Compile Ailu DSL graph YAML into a validated graph definition.

    Args:
        yaml: The graph DSL document as a string.

    Returns:
        The compiled ``GraphDefinition`` as a ``dict``.

    Raises:
        GraphCompileError: On parse, DSL, or structural validation failure.
    """
    try:
        result = _native.compile_graph_yaml(yaml)
    except ValueError as error:
        raise GraphCompileError(str(error)) from error
    return json.loads(result)


# ---------------------------------------------------------------------------
# Model policy
# ---------------------------------------------------------------------------


def available_providers() -> List[str]:
    """Return the LLM providers usable in the current process environment.

    The set is derived from env credentials by the Rust engine
    (``ModelPolicy::available_from_env``): e.g. ``MISTRAL_API_KEY`` enables
    ``"mistral"``, ``ANTHROPIC_API_KEY`` enables ``"anthropic"``, and
    ``AILU_USE_OLLAMA=1`` enables ``"ollama"``.

    Returns:
        A list of provider id strings (empty when no credentials are present).
    """
    return json.loads(_native.available_providers())


def resolve_model(
    tier: str,
    available: Optional[List[str]] = None,
    *,
    provider: Optional[str] = None,
    model: Optional[str] = None,
) -> Dict[str, Any]:
    """Resolve a capability tier to a concrete model choice.

    Args:
        tier: A capability tier — one of ``"frontier"``, ``"balanced"``,
            ``"fast"``, or ``"creative"``.
        available: The providers to choose among. When ``None``, the providers
            are derived from the process environment (see
            :func:`available_providers`).
        provider: Optional provider override. When set, it wins over the policy
            choice and the result is flagged ``recommended = False``.
        model: Optional model override (same override semantics as ``provider``).

    Returns:
        A ``ModelChoice`` dict: ``{"provider": str, "model": str,
        "recommended": bool}``.

    Raises:
        ValueError: On an unknown tier, an unknown provider, or a malformed
            override at the JSON boundary.
    """
    available_json = None if available is None else json.dumps(available)
    override = {}
    if provider is not None:
        override["provider"] = provider
    if model is not None:
        override["model"] = model
    override_json = json.dumps(override) if override else None
    result = _native.resolve_model(tier, available_json, override_json)
    return json.loads(result)


# ---------------------------------------------------------------------------
# Catalogs
# ---------------------------------------------------------------------------


def list_components() -> List[str]:
    """Return the component kinds the engine knows how to build.

    Returns:
        A list of component-kind strings (e.g. ``"promptBuilder"``).
    """
    return json.loads(_native.list_components())


def list_prebuilt() -> List[Dict[str, Any]]:
    """Return every prebuilt micro-agent definition.

    Returns:
        A list of ``PrebuiltAgent`` dicts (camelCase keys: ``name``,
        ``description``, ``tier``, ``systemPrompt``, ``toolNames``,
        ``suspendForApproval``, ``outputChannel``).
    """
    return json.loads(_native.list_prebuilt())


# ---------------------------------------------------------------------------
# Run paths (fully on Rust)
# ---------------------------------------------------------------------------


def run_component(kind: str, params: Dict[str, Any], channels: Dict[str, Any]) -> Dict[str, Any]:
    """Run a single component handler, fully on Rust.

    Args:
        kind: The component kind to build (see :func:`list_components`).
        params: The component's configuration (e.g.
            ``{"template": "Hi {{name}}!", "into": "prompt"}``).
        channels: The initial channel snapshot the handler reads from.

    Returns:
        The component's channel-update map (its output patch) as a dict.

    Raises:
        RunError: On an unknown kind, invalid params/channels, or a handler
            failure reported by the engine.
    """
    try:
        params_json = json.dumps(params)
        channels_json = json.dumps(channels)
    except (TypeError, ValueError) as error:
        raise RunError(f"params/channels are not JSON-serialisable: {error}") from error
    try:
        result = _native.run_component(kind, params_json, channels_json)
    except ValueError as error:
        raise RunError(str(error)) from error
    return json.loads(result)


def run_prebuilt(
    name: str,
    input: Any,
    *,
    provider: Optional[str] = None,
    model: Optional[str] = None,
) -> Dict[str, Any]:
    """Run a prebuilt micro-agent, fully on Rust.

    The agent's model is resolved from its tier and the env-available providers
    (honouring the optional ``provider``/``model`` override); the gateway falls
    back to a deterministic mock when no provider credentials are present, so a
    run still completes offline.

    Args:
        name: The prebuilt agent name (see :func:`list_prebuilt`).
        input: The agent input (any JSON-serialisable value), seeded into the
            run's ``input`` channel.
        provider: Optional provider override for model resolution.
        model: Optional model override for model resolution.

    Returns:
        A ``RunOutcome`` dict: ``{"status": str, "channels": dict,
        "resolvedModel": {"provider": str, "model": str}}``.

    Raises:
        RunError: On an unknown agent name, invalid input, or a runtime error.
    """
    try:
        input_json = json.dumps(input)
    except (TypeError, ValueError) as error:
        raise RunError(f"input is not JSON-serialisable: {error}") from error
    options = {}
    if provider is not None:
        options["provider"] = provider
    if model is not None:
        options["model"] = model
    options_json = json.dumps(options) if options else None
    try:
        result = _native.run_prebuilt(name, input_json, options_json)
    except ValueError as error:
        raise RunError(str(error)) from error
    return json.loads(result)


class _PrebuiltAccessor:
    """Ergonomic accessor for the prebuilt micro-agents.

    Each attribute resolves to a callable bound to that agent name, so::

        ailu.prebuilt.summarizer("some long text")

    is shorthand for ``ailu.run_prebuilt("summarizer", "some long text")``.
    Any attribute name is accepted; an unknown agent surfaces as a
    :class:`RunError` only when the returned callable is invoked.
    """

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)

        def _run(input: Any, *, provider: Optional[str] = None, model: Optional[str] = None):
            return run_prebuilt(name, input, provider=provider, model=model)

        _run.__name__ = name
        _run.__qualname__ = f"prebuilt.{name}"
        _run.__doc__ = f"Run the '{name}' prebuilt agent (see run_prebuilt)."
        return _run

    def __dir__(self):
        return [agent["name"] for agent in list_prebuilt()]


prebuilt = _PrebuiltAccessor()
"""Ergonomic accessor: ``ailu.prebuilt.<agent_name>(input, ...)``."""
