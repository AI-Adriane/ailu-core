"""A graph builder for Python — the TypeScript ``createGraph`` counterpart (ADR 0045 M4).

The builder writes the ``GraphDefinition`` the TypeScript builder writes for the same calls:
agents and components are settings on their nodes (``metadata.agent``,
``metadata.component``), steps are plain action nodes, and the agent output and approval
channels are declared for you. So a graph built here can be saved and run from TypeScript, and
the reverse. Running it goes through the engine's catalog spec, as :func:`ailu.run_catalog_graph`
does, with your functions as the steps, the tools and the conditions.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Literal, Mapping, Optional, Sequence

from . import (
    GraphCompileError,
    HostNode,
    _catalog_runner,
    _children,
    _ensure_approvals_granted,
    _resumed,
    _started,
    explain_run,
    validate_graph,
)

DEFAULT_AGENT_OUTPUT_CHANNEL = "agentResult"
"""The channel an agent writes its result into when it names none."""

_APPROVAL_CHANNELS = ("__approvedTools", "__approvalIds")

_UNSET: Any = object()


@dataclass(frozen=True)
class Tool:
    """A tool an agent may call.

    Attributes:
        fn: Called with the tool's input; returns its result.
        description: What the model is told the tool does.
        input_schema: The JSON Schema of its input, as the model sees it.
        requires_approval: The agent asks a person before using it (with
            ``suspend_for_approval``, the run suspends until a person decides).
        approval_when: With ``requires_approval``, ask only for the calls that
            cross a condition, each with one test: above a threshold (ADR 0046),
            ``[{"argument": "amount", "above": 500}]``, or one of named values
            (ADR 0048), ``[{"argument": "agentName", "in": ["Nordlys"]}]``. The
            engine opens the gate when an argument is absent, not of its test's
            type, above its threshold or one of its values, and the grant is that
            call — never the tool for the rest of the run.
    """

    fn: Callable[[Any], Any]
    description: Optional[str] = None
    input_schema: Optional[Mapping[str, Any]] = None
    requires_approval: bool = False
    approval_when: Optional[Sequence[Mapping[str, Any]]] = None


def _approval_condition(condition: Mapping[str, Any]) -> Dict[str, Any]:
    """A condition as the carrier writes it: its argument and the test it carries,
    ``above`` (ADR 0046) or ``in`` (ADR 0048). The engine refuses one with both
    or neither when it builds the agent — nothing is dropped here."""
    written: Dict[str, Any] = {"argument": condition["argument"]}
    if "above" in condition:
        written["above"] = condition["above"]
    if "in" in condition:
        written["in"] = list(condition["in"])
    return written


def _slug(name: str) -> str:
    """The TypeScript builder's graph id for a name."""
    return re.sub(r"(^-|-$)", "", re.sub(r"[^a-z0-9]+", "-", name.strip().lower())) or "graph"


class GraphBuilder:
    """Build a graph step by step; :meth:`compile` validates it on the engine.

    Use :func:`create_graph`. Each method returns the builder, so calls chain.
    The first node added is the entry, unless :meth:`entry` names another.
    """

    def __init__(
        self,
        name: str,
        *,
        id: Optional[str] = None,  # noqa: A002 - the definition's field
        version: str = "0.0.0",
        recursion_limit: Optional[int] = None,
        metadata: Optional[Mapping[str, Any]] = None,
    ) -> None:
        self._header: Dict[str, Any] = {"id": id or _slug(name), "version": version, "name": name}
        if recursion_limit is not None:
            self._header["recursionLimit"] = recursion_limit
        self._metadata = dict(metadata) if metadata is not None else None
        self._channels: Dict[str, Dict[str, Any]] = {}
        self._nodes: List[Dict[str, Any]] = []
        self._edges: List[Dict[str, Any]] = []
        self._entry: Optional[str] = None
        self._steps: Dict[str, HostNode] = {}
        self._tools: Dict[str, Callable[[Any], Any]] = {}
        self._conditions: Dict[str, Callable[[Dict[str, Any]], bool]] = {}
        self._subgraphs: Dict[str, Dict[str, Any]] = {}
        self._fs_policy: List[Dict[str, Any]] = []

    # -- channels and nodes ---------------------------------------------------

    def channel(
        self,
        name: str,
        type: str,  # noqa: A002 - the channel's field
        *,
        default: Any = _UNSET,
        reducer: str = "replace",
        no_log: bool = False,
    ) -> "GraphBuilder":
        """Declare a channel of the run's state.

        Args:
            name: The channel's name.
            type: Its type label (``"string"``, ``"json"``, ``"number"``, …).
            default: Its initial value.
            reducer: How an update combines with it: ``"replace"``,
                ``"append"`` or ``"merge"``.
            no_log: Keep its value out of run events (still checkpointed).
        """
        channel: Dict[str, Any] = {"type": type, "reducer": reducer}
        if default is not _UNSET:
            channel["default"] = default
        if no_log:
            channel["noLog"] = True
        self._channels[name] = channel
        return self

    def _ensure_channel(self, name: str, channel: Dict[str, Any]) -> None:
        self._channels.setdefault(name, channel)

    def _push(
        self,
        node_id: str,
        node_type: str,
        label: Optional[str],
        extra: Optional[Dict[str, Any]] = None,
    ) -> None:
        if any(node["id"] == node_id for node in self._nodes):
            raise ValueError(f"the graph already has a node {node_id!r}")
        self._nodes.append(
            {"id": node_id, "type": node_type, "label": label or node_id, **(extra or {})}
        )
        if self._entry is None:
            self._entry = node_id

    def node(
        self,
        node_id: str,
        fn: HostNode,
        *,
        label: Optional[str] = None,
        retry_policy: Optional[Mapping[str, int]] = None,
    ) -> "GraphBuilder":
        """Add a step: ``fn(node: HostNodeInput) -> dict`` returns the channel update.

        It runs as a host node: journaled when recording, never called again by a
        replay. ``retry_policy`` is ``{"maxAttempts", "backoffMs"}``.
        """
        extra = {"retryPolicy": dict(retry_policy)} if retry_policy is not None else None
        self._push(node_id, "action", label, extra)
        self._steps[node_id] = fn
        return self

    def human_gate(self, node_id: str, *, label: Optional[str] = None) -> "GraphBuilder":
        """Add a gate: the run suspends there until it is resumed (a person approved)."""
        self._push(node_id, "human-gate", label)
        return self

    def agent_node(
        self,
        node_id: str,
        *,
        system: str,
        provider: str = "anthropic",
        model: Optional[str] = None,
        tier: Optional[str] = None,
        tools: Optional[Mapping[str, Any]] = None,
        max_iterations: Optional[int] = None,
        output_channel: str = DEFAULT_AGENT_OUTPUT_CHANNEL,
        suspend_for_approval: bool = False,
        approval_scope: Optional[Literal["tool", "call"]] = None,
        visible_channels: Optional[List[str]] = None,
        output_style: Optional[str] = None,
        context_budget: Optional[int] = None,
        label: Optional[str] = None,
    ) -> "GraphBuilder":
        """Add an agent: a model that reasons and calls tools, run by the engine.

        Args:
            node_id: The node's id.
            system: The system prompt.
            provider: The model provider. ``""`` with a ``tier`` lets the engine
                pick among the keys set (the TypeScript ``model.fast``).
            model: The provider's model id.
            tier: ``"fast"``, ``"balanced"``, ``"frontier"`` or ``"creative"``,
                resolved by the engine when no ``model`` is given.
            tools: ``{name: fn}`` or ``{name: Tool(...)}``.
            max_iterations: Upper bound on the agent's reasoning steps.
            output_channel: Where its result lands (declared for you).
            suspend_for_approval: A tool that ``requires_approval`` suspends the
                run until a person decides.
            approval_scope: What one approval of the agent's gated tools unlocks
                (ADR 0051 D4): ``"tool"`` (the default) or ``"call"`` — the grant
                is that one call, spent once it runs; another call, or the same one
                again, waits for a new approval. The engine refuses another value.
            visible_channels: The only channels the agent is shown.
            output_style: ``"terse"`` asks for compact answers.
            context_budget: Cap, in characters, on the state the agent is shown.
            label: A display label.
        """
        named = {
            name: tool if isinstance(tool, Tool) else Tool(tool)
            for name, tool in (tools or {}).items()
        }
        for name, tool in named.items():
            known = self._tools.get(name)
            if known is not None and known is not tool.fn:
                raise ValueError(f"two different tools are named {name!r}")
            self._tools[name] = tool.fn
        specs = []
        for name, tool in named.items():
            spec: Dict[str, Any] = {"name": name}
            if tool.description is not None:
                spec["description"] = tool.description
            if tool.input_schema is not None:
                spec["jsonSchema"] = dict(tool.input_schema)
            specs.append(spec)
        middleware: List[Dict[str, Any]] = []
        if output_style == "terse":
            middleware.append({"kind": "terse"})
        if context_budget is not None:
            middleware.append({"kind": "contextBudget", "params": {"chars": context_budget}})
        carrier: Dict[str, Any] = {"provider": provider}
        for key, value in (("model", model), ("tier", tier), ("system", system)):
            if value is not None:
                carrier[key] = value
        carrier["toolNames"] = list(named)
        carrier["toolSpecs"] = specs
        if max_iterations is not None:
            carrier["maxIterations"] = max_iterations
        carrier["suspendForApproval"] = suspend_for_approval
        carrier["approvalToolNames"] = [
            name for name, tool in named.items() if tool.requires_approval
        ]
        approval_when = {
            name: [_approval_condition(condition) for condition in tool.approval_when]
            for name, tool in named.items()
            if tool.approval_when
        }
        if approval_when:
            carrier["approvalWhen"] = approval_when
        if approval_scope is not None:
            carrier["approvalScope"] = approval_scope
        carrier["outputChannel"] = output_channel
        if output_style is not None:
            carrier["outputStyle"] = output_style
        if context_budget is not None:
            carrier["contextBudget"] = context_budget
        if visible_channels is not None:
            carrier["visibleChannels"] = list(visible_channels)
        carrier["resolvedMiddleware"] = middleware
        self._push(node_id, "agent", label, {"metadata": {"agent": carrier}})
        self._ensure_channel(output_channel, {"type": "agentResult", "reducer": "replace"})
        for channel in _APPROVAL_CHANNELS:
            self._ensure_channel(channel, {"type": "string[]", "reducer": "replace", "default": []})
        return self

    def component(
        self,
        node_id: str,
        kind: str,
        params: Optional[Mapping[str, Any]] = None,
        *,
        label: Optional[str] = None,
    ) -> "GraphBuilder":
        """Add a component the engine runs natively (``promptBuilder``, ``retriever``, …).

        :func:`ailu.list_components` lists the kinds; the engine validates
        ``params`` when the run starts.
        """
        metadata = {"component": {"kind": kind, "params": dict(params or {})}}
        self._push(node_id, "action", label, {"metadata": metadata})
        return self

    def subgraph(
        self,
        node_id: str,
        child: "GraphBuilder",
        *,
        input_mapping: Optional[Mapping[str, str]] = None,
        output_mapping: Optional[Mapping[str, str]] = None,
        label: Optional[str] = None,
    ) -> "GraphBuilder":
        """Nest another builder's graph as one node.

        ``input_mapping`` (``{child_channel: parent_channel}``) projects the
        parent's channels in; ``output_mapping`` (``{parent_channel:
        child_channel}``) brings the child's back. A child that suspends
        suspends the parent. Its steps, tools and conditions join this graph's:
        node ids must be unique across both.
        """
        definition = child._definition()
        for node_id_in_child in child._steps:
            if node_id_in_child in self._steps or any(
                n["id"] == node_id_in_child for n in self._nodes
            ):
                raise ValueError(f"the graph already has a node {node_id_in_child!r}")
        self._steps.update(child._steps)
        for name, fn in child._tools.items():
            known = self._tools.get(name)
            if known is not None and known is not fn:
                raise ValueError(f"two different tools are named {name!r}")
            self._tools[name] = fn
        self._conditions.update(child._conditions)
        self._subgraphs[definition["id"]] = definition
        self._subgraphs.update(child._subgraphs)
        extra: Dict[str, Any] = {"subgraphId": definition["id"]}
        if input_mapping is not None:
            extra["inputMapping"] = dict(input_mapping)
        if output_mapping is not None:
            extra["outputMapping"] = dict(output_mapping)
        self._push(node_id, "subgraph", label, extra)
        return self

    # -- edges ------------------------------------------------------------------

    def _edge(
        self, source: str, target: str, edge_type: str, condition: Optional[str] = None
    ) -> None:
        edge: Dict[str, Any] = {
            "id": f"e_{source}_{target}_{len(self._edges)}",
            "from": source,
            "to": target,
            "type": edge_type,
        }
        if condition is not None:
            edge["condition"] = condition
        self._edges.append(edge)

    def edge(self, source: str, target: str) -> "GraphBuilder":
        """Go from ``source`` to ``target``."""
        self._edge(source, target, "default")
        return self

    def conditional_edge(
        self,
        source: str,
        target: str,
        name: str,
        predicate: Callable[[Dict[str, Any]], bool],
    ) -> "GraphBuilder":
        """Go from ``source`` to ``target`` when ``predicate(channels)`` is true.

        ``name`` is how the saved graph refers to the condition.
        """
        self._conditions[name] = predicate
        self._edge(source, target, "conditional", name)
        return self

    def error_edge(self, source: str, target: str) -> "GraphBuilder":
        """Go to ``target`` once ``source`` has failed after its retries."""
        self._edge(source, target, "error")
        return self

    def entry(self, node_id: str) -> "GraphBuilder":
        """Start the run at ``node_id`` (default: the first node added)."""
        self._entry = node_id
        return self

    def fs_policy(self, rules: List[Mapping[str, str]]) -> "GraphBuilder":
        """Per-path rules (``{"glob", "verb"}``) for agents using the governed filesystem."""
        self._fs_policy.extend(dict(rule) for rule in rules)
        return self

    # -- compile ------------------------------------------------------------------

    def _definition(self) -> Dict[str, Any]:
        definition: Dict[str, Any] = {
            **self._header,
            "channels": {name: dict(channel) for name, channel in self._channels.items()},
            "nodes": [dict(node) for node in self._nodes],
            "edges": [dict(edge) for edge in self._edges],
            "entryNodeId": self._entry or "",
        }
        if self._metadata is not None:
            definition["metadata"] = dict(self._metadata)
        return definition

    def compile(self) -> "CompiledGraph":
        """Validate the graph and its subgraphs on the engine.

        Raises:
            GraphCompileError: With every problem the engine found in ``errors``.
        """
        definition = self._definition()
        subgraphs = list(self._subgraphs.values())
        errors = [error for graph in (definition, *subgraphs) for error in validate_graph(graph)]
        if errors:
            summary = "; ".join(f"{error['code']}: {error['message']}" for error in errors)
            failure = GraphCompileError(f"the graph failed to compile: {summary}")
            failure.errors = errors  # type: ignore[attr-defined]
            raise failure
        return CompiledGraph(
            definition,
            subgraphs=subgraphs,
            steps=dict(self._steps),
            tools=dict(self._tools),
            conditions=dict(self._conditions),
            fs_policy=list(self._fs_policy),
        )


def create_graph(
    name: str,
    *,
    id: Optional[str] = None,  # noqa: A002 - the definition's field
    version: str = "0.0.0",
    recursion_limit: Optional[int] = None,
    metadata: Optional[Mapping[str, Any]] = None,
) -> GraphBuilder:
    """Start building a graph (the TypeScript ``createGraph``).

    ``id`` defaults to the name, lowercased with dashes (``"Support Flow"`` →
    ``"support-flow"``), as in TypeScript.
    """
    return GraphBuilder(
        name, id=id, version=version, recursion_limit=recursion_limit, metadata=metadata
    )


class CompiledGraph:
    """A validated graph, ready to run on the engine with its functions.

    ``definition`` (and ``subgraphs``) is plain data: save it, render it, or run
    it from TypeScript with ``runCatalogGraph``. Every method returns the run
    outcome — ``state``, ``status``, ``pendingApprovals``, and when recording
    ``replayJournal`` and ``entryState`` — as :func:`ailu.run_catalog_graph`
    does. State lives in the outcome, not in this object: keep it to resume.
    """

    def __init__(
        self,
        definition: Dict[str, Any],
        *,
        subgraphs: List[Dict[str, Any]],
        steps: Dict[str, HostNode],
        tools: Dict[str, Callable[[Any], Any]],
        conditions: Dict[str, Callable[[Dict[str, Any]], bool]],
        fs_policy: List[Dict[str, Any]],
    ) -> None:
        self.definition = definition
        self.subgraphs = subgraphs
        self._steps = steps
        self._tools = tools
        self._conditions = conditions
        self._fs_policy = fs_policy

    def _runner(
        self,
        *,
        on_event: Optional[Callable[[Dict[str, Any]], None]],
        provider_keys: Optional[Mapping[str, str]],
        skills: Optional[List[Mapping[str, Any]]],
        with_functions: bool = True,
    ):
        return _catalog_runner(
            self.definition,
            nodes=self._steps if with_functions else None,
            tools=self._tools if with_functions else None,
            subgraphs=self.subgraphs,
            provider_keys=provider_keys,
            fs_policy=self._fs_policy,
            skills=skills,
            on_event=on_event,
            conditions=self._conditions,
        )

    def run(
        self,
        initial_data: Optional[Mapping[str, Any]] = None,
        *,
        run_id: Optional[str] = None,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
        stream_tokens: bool = False,
        approval_engine: Any = None,
        provider_keys: Optional[Mapping[str, str]] = None,
        skills: Optional[List[Mapping[str, Any]]] = None,
        checkpointer: Any = None,
    ) -> Dict[str, Any]:
        """Start a run (options as for :func:`ailu.run_catalog_graph`)."""
        runner = self._runner(on_event=on_event, provider_keys=provider_keys, skills=skills)
        outcome = runner.run(
            initial_data,
            run_id=run_id,
            is_cancelled=is_cancelled,
            stream_tokens=stream_tokens,
            checkpointer=checkpointer,
        )
        return _started(self.definition, _children(self.subgraphs), outcome, approval_engine)

    def resume(
        self,
        state: Mapping[str, Any],
        *,
        approved_tools: Optional[List[Mapping[str, Any]]] = None,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
        approval_engine: Any = None,
        provider_keys: Optional[Mapping[str, str]] = None,
        skills: Optional[List[Mapping[str, Any]]] = None,
        checkpointer: Any = None,
    ) -> Dict[str, Any]:
        """Resume a suspended run (options as for :func:`ailu.resume_catalog_graph`)."""
        children = _children(self.subgraphs)
        if approval_engine is not None:
            _ensure_approvals_granted(
                self.definition, children, state, approval_engine, approved_tools or []
            )
        runner = self._runner(on_event=on_event, provider_keys=provider_keys, skills=skills)
        outcome = runner.resume(
            state,
            approved_tools=approved_tools,
            is_cancelled=is_cancelled,
            checkpointer=checkpointer,
        )
        return _resumed(self.definition, children, outcome, state, approval_engine)

    def signal(
        self,
        state: Mapping[str, Any],
        name: str,
        payload: Any = None,
        *,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
        is_cancelled: Optional[Callable[[], bool]] = None,
        provider_keys: Optional[Mapping[str, str]] = None,
        skills: Optional[List[Mapping[str, Any]]] = None,
        checkpointer: Any = None,
    ) -> Dict[str, Any]:
        """Deliver the signal ``name`` to a run waiting on it, then resume it."""
        runner = self._runner(on_event=on_event, provider_keys=provider_keys, skills=skills)
        return runner.signal(
            state, name, payload, is_cancelled=is_cancelled, checkpointer=checkpointer
        )

    def replay(
        self,
        state: Mapping[str, Any],
        checkpoint_id: str,
        replay_journal: str,
        *,
        on_event: Optional[Callable[[Dict[str, Any]], None]] = None,
        provider_keys: Optional[Mapping[str, str]] = None,
        skills: Optional[List[Mapping[str, Any]]] = None,
    ) -> Dict[str, Any]:
        """Re-derive a recorded run: nothing of yours is called (steps and tools
        come from the recording); conditions are evaluated, as they route it."""
        runner = self._runner(
            on_event=on_event, provider_keys=provider_keys, skills=skills, with_functions=False
        )
        return runner.replay(state, checkpoint_id, replay_journal)

    def explain(
        self, state: Mapping[str, Any], events: Optional[List[Mapping[str, Any]]] = None
    ) -> Dict[str, Any]:
        """Where a run of this graph stands (:func:`ailu.explain_run`)."""
        return explain_run(state, events)
