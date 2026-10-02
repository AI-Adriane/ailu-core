"""Ailu Python SDK.

A thin, Pythonic wrapper over the Ailu Rust engine, exposed through a native
pyo3 extension module. This SDK and the TypeScript SDK share ONE Rust engine —
the same graph validator and DSL compiler back both languages, so behaviour is
identical across the ecosystem (the multi-language-SDK strategy: one engine,
thin per-language SDKs).

The native extension (`ailu.ailu`, built from `crates/py-bindings`)
speaks JSON in / JSON out. This module hides that boundary, taking and returning
native Python `dict`/`list` values.

:class:`GraphRunner` runs a graph on the engine with your code as steps (host
nodes), tools, conditions, event handlers and a cancellation check — the same
runner the TypeScript SDK drives. A replay never calls your steps or tools: the
engine serves what the recorded run returned.
"""

from __future__ import annotations

import inspect
import json
import uuid
from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Mapping, Optional

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
    "GraphRunner",
    "HostNodeInput",
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
    """Raised when a component, prebuilt-agent or graph run fails.

    Covers an unknown component kind / agent name, invalid params or input, an
    engine spec the engine refuses, a replay that diverged from its recording,
    and a handler/runtime failure reported by the Rust engine.
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


# ---------------------------------------------------------------------------
# Graph runner (ADR 0045 D2.2)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class HostNodeInput:
    """What a host node receives for one execution.

    Attributes:
        node_id: The node's id.
        channels: The run's channels when the node starts.
        effect_key: sha256 of (run id, node id, state version at the node's
            entry). A retry of the same step from the same checkpoint gets the
            same key; the next run, or the next pass in a loop, gets another.
            Perform an external effect (a message sent, a record written) at
            most once per key.
    """

    node_id: str
    channels: Dict[str, Any]
    effect_key: str


HostNode = Callable[[HostNodeInput], Optional[Mapping[str, Any]]]
"""A host node: receives a :class:`HostNodeInput`, returns the channel update."""


def _synchronous(value: Any, what: str) -> Any:
    """Refuse an awaitable: host functions are called synchronously."""
    if inspect.isawaitable(value):
        close = getattr(value, "close", None)
        if callable(close):
            close()  # no "never awaited" warning for a coroutine we refuse
        raise TypeError(f"{what} returned an awaitable; host functions must be synchronous")
    return value


class GraphRunner:
    """Run a graph on the Rust engine, with your code as steps and tools.

    The runner takes the static part of an ``EngineSpec`` — at least ``graph``,
    and any of ``subgraphs``, ``agents``, ``componentNodes``, ``mapAgents``,
    ``providerKeys``, ``fsPolicy``, ``skills`` (camelCase keys, as the engine
    reads them) — plus your functions:

    * ``nodes``: ``{node_id: fn(HostNodeInput) -> dict}``. A plain action node
      whose step is your code; its return value is the channel update. The
      engine journals each execution when recording (``AILU_LLM_RECORD=1``) and
      a replay serves the journal instead of calling it again.
    * ``tools``: ``{name: fn(input) -> result}``. A tool an agent may call.
    * ``conditions``: ``{name: fn(channels) -> bool}``. The named conditions of
      the graph's conditional edges. A missing one fails the run rather than
      route it down a branch by default.
    * ``on_event``: ``fn(event_dict)``, called for every run lifecycle event.

    Each call returns the run outcome as a dict: ``state`` (the graph state,
    channels included), ``status`` (``"completed"``, ``"suspended"``,
    ``"failed"`` or ``"cancelled"``), ``pendingApprovals``, and when recording
    ``replayJournal`` (a JSON string) and ``entryState`` (on a start) — store
    those two to replay the run later.

    Functions are called synchronously, on the calling thread. An exception in a
    step or a tool fails it, and the run unless the graph handles the failure.

    Raises:
        RunError: When the engine refuses the spec, or a replay diverges from
            its recording.
    """

    def __init__(
        self,
        spec: Mapping[str, Any],
        *,
        nodes: Optional[Mapping[str, HostNode]] = None,
        tools: Optional[Mapping[str, Callable[[Any], Any]]] = None,
        conditions: Optional[Mapping[str, Callable[[Dict[str, Any]], bool]]] = None,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
    ) -> None:
        if "graph" not in spec:
            raise ValueError("the engine spec needs a 'graph'")
        self._nodes: Dict[str, HostNode] = dict(nodes or {})
        self._tools: Dict[str, Callable[[Any], Any]] = dict(tools or {})
        self._conditions = dict(conditions or {})
        self._on_event = on_event
        base = dict(spec)
        # `jsNodeIds` is the older name of `hostNodeIds`; the engine refuses both at once.
        host_node_ids = set(base.pop("jsNodeIds", None) or []) | set(
            base.pop("hostNodeIds", None) or []
        )
        base["hostNodeIds"] = sorted(host_node_ids | set(self._nodes))
        base["jsToolNames"] = sorted(set(base.get("jsToolNames") or []) | set(self._tools))
        self._base = base

    def run(
        self,
        initial_data: Optional[Mapping[str, Any]] = None,
        *,
        run_id: Optional[str] = None,
        inbox: Optional[Mapping[str, List[Any]]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict[str, Any]:
        """Start a run.

        Args:
            initial_data: Channel values seeding the run.
            run_id: A stable run id. Defaults to a generated one.
            inbox: Inputs to queue per node id (``send``).
            is_cancelled: Polled at every node boundary; returning ``True``
                stops the run there with status ``"cancelled"``, its last
                checkpoint intact.
        """
        spec = {
            **self._base,
            "runId": run_id or f"run_{uuid.uuid4()}",
            "initialData": dict(initial_data or {}),
            "inbox": dict(inbox or {}),
        }
        return self._call(_native.engine_run, spec, is_cancelled)

    def resume(
        self,
        state: Mapping[str, Any],
        *,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict[str, Any]:
        """Resume a suspended run from its state (past a gate, a timer, an interrupt)."""
        return self._call(_native.engine_resume, {**self._base, "state": dict(state)}, is_cancelled)

    def approve_and_resume(
        self,
        state: Mapping[str, Any],
        approved_tools: List[Mapping[str, Any]],
        *,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict[str, Any]:
        """Grant tools, then resume.

        Args:
            state: The suspended run's state.
            approved_tools: ``[{"name", "requestedBy", "resolvedBy"}]``. The
                engine refuses a grant whose approver is empty or requested it.
            is_cancelled: As for :meth:`run`.
        """
        spec = {
            **self._base,
            "state": dict(state),
            "approvedTools": [dict(tool) for tool in approved_tools],
        }
        return self._call(_native.engine_approve_and_resume, spec, is_cancelled)

    def signal(
        self,
        state: Mapping[str, Any],
        name: str,
        payload: Any = None,
        *,
        is_cancelled: Optional[Callable[[], bool]] = None,
    ) -> Dict[str, Any]:
        """Deliver signal ``name`` to a run waiting on it, then resume it."""
        spec = {**self._base, "state": dict(state)}
        return self._call(
            lambda spec_json, *callbacks: _native.engine_signal(
                spec_json, name, json.dumps(payload), *callbacks
            ),
            spec,
            is_cancelled,
        )

    def replay(
        self, state: Mapping[str, Any], checkpoint_id: str, replay_journal: str
    ) -> Dict[str, Any]:
        """Re-derive a recorded run from ``state`` with its ``replay_journal``.

        Model outputs, tool results and step results come from the recording:
        no model, tool or step of yours is called. Conditions are evaluated, as
        they route the run. A replay that reaches what its recording has no
        result for diverged: it raises :class:`RunError`.
        """
        spec = {**self._base, "state": dict(state), "replayJournal": replay_journal}
        try:
            result = _native.engine_replay(
                json.dumps(spec),
                checkpoint_id,
                self._stub_node,
                self._on_condition,
                self._forward_event,
            )
        except ValueError as error:
            raise RunError(str(error)) from error
        return json.loads(result)

    # -- callbacks ----------------------------------------------------------

    def _call(
        self,
        entry: Callable[..., str],
        spec: Dict[str, Any],
        is_cancelled: Optional[Callable[[], bool]],
    ):
        try:
            result = entry(
                json.dumps(spec),
                self._on_node,
                self._on_condition,
                self._forward_event,
                is_cancelled,
            )
        except ValueError as error:
            raise RunError(str(error)) from error
        return json.loads(result)

    def _on_node(self, payload_json: str) -> str:
        payload = json.loads(payload_json)
        if payload.get("kind") == "node":
            node_id = payload.get("nodeId")
            step = self._nodes.get(node_id)
            if step is None:
                return "{}"  # a plain node without a function: an empty step
            effect_key = payload.get("effectKey")
            if not effect_key:
                raise RunError(f"the engine sent no effect key for host node {node_id!r}")
            update = _synchronous(
                step(
                    HostNodeInput(
                        node_id=node_id,
                        channels=dict(payload.get("state") or {}),
                        effect_key=effect_key,
                    )
                ),
                f"host node {node_id!r}",
            )
            if update is None:
                return "{}"
            if not isinstance(update, Mapping):
                kind = type(update).__name__
                raise TypeError(f"host node {node_id!r} must return a dict of updates, not {kind}")
            return json.dumps(dict(update))
        name = payload.get("name")
        tool = self._tools.get(name)
        if tool is None:
            return "{}"
        return json.dumps(_synchronous(tool(payload.get("input")), f"tool {name!r}"))

    @staticmethod
    def _stub_node(payload_json: str) -> str:
        # A replay serves steps and tools from its recording; nothing of yours runs.
        return "{}"

    def _on_condition(self, payload_json: str) -> bool:
        payload = json.loads(payload_json)
        name = payload.get("name")
        predicate = self._conditions.get(name)
        if predicate is None:
            raise LookupError(f"no condition function named {name!r}: pass it in conditions=")
        return bool(
            _synchronous(predicate(dict(payload.get("state") or {})), f"condition {name!r}")
        )

    def _forward_event(self, event_json: str) -> None:
        if self._on_event is not None:
            self._on_event(json.loads(event_json))
