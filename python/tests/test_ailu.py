"""Tests for the Ailu Python SDK.

Designed to run two ways:

  * ``pytest`` (collects the ``test_*`` functions), and
  * ``python -m tests.test_ailu`` / ``python tests/test_ailu.py`` when
    pytest is not installed — the ``__main__`` block runs every test with plain
    asserts and reports a pass/fail summary.

Both paths exercise the same Rust engine that backs the TypeScript SDK.
"""

from __future__ import annotations

import json
import os
import sys
import warnings

# Make the package importable when run as a bare script (no pytest / no install).
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import ailu

_VALID_GRAPH = {
    "id": "g",
    "version": "0.0.0",
    "name": "g",
    "channels": {},
    "nodes": [{"id": "a", "type": "action", "label": "a"}],
    "edges": [],
    "entryNodeId": "a",
}

_DANGLING_EDGE_GRAPH = {
    "id": "g",
    "version": "0.0.0",
    "name": "g",
    "channels": {},
    "nodes": [{"id": "a", "type": "action", "label": "a"}],
    "edges": [{"id": "e1", "from": "a", "to": "ghost", "type": "default"}],
    "entryNodeId": "a",
}

_TINY_YAML = (
    "id: g\n"
    "version: 0.0.0\n"
    "name: g\n"
    "entryNodeId: a\n"
    "nodes:\n"
    "  - id: a\n"
    "    type: action\n"
    "    label: A\n"
    "edges: []\n"
    "channels: {}\n"
)


def test_engine_version_is_version_string():
    version = ailu.engine_version()
    assert isinstance(version, str)
    assert version != ""
    # Looks like a semver-ish "x.y.z".
    parts = version.split(".")
    assert len(parts) >= 3, version
    assert all(p.isdigit() for p in parts[:3]), version


def test_validate_graph_valid_returns_empty():
    errors = ailu.validate_graph(_VALID_GRAPH)
    assert errors == [], errors


def test_validate_graph_dangling_edge_flags_invalid_edge_reference():
    errors = ailu.validate_graph(_DANGLING_EDGE_GRAPH)
    assert isinstance(errors, list)
    assert len(errors) >= 1, errors
    codes = [e["code"] for e in errors]
    assert "INVALID_EDGE_REFERENCE" in codes, codes
    # The flagged edge id surfaces in the error path.
    offending = next(e for e in errors if e["code"] == "INVALID_EDGE_REFERENCE")
    assert "e1" in offending["path"], offending


def test_compile_graph_yaml_returns_dict_with_expected_shape():
    graph = ailu.compile_graph_yaml(_TINY_YAML)
    assert isinstance(graph, dict)
    assert graph["id"] == "g"
    assert graph["entryNodeId"] == "a"
    node_ids = [n["id"] for n in graph["nodes"]]
    assert node_ids == ["a"], graph["nodes"]


def test_compile_graph_yaml_raises_on_garbage():
    raised = False
    try:
        ailu.compile_graph_yaml("this: is: not: a: graph: ::::")
    except ValueError:
        raised = True
    assert raised, "expected a ValueError on malformed DSL YAML"


def _force_mock_env():
    """Drop every provider credential from the process env and turn offline mode on
    (``AILU_LLM_MOCK=1``) so the engine resolves to the deterministic mock gateway
    (these run paths read ``std::env`` live)."""
    for key in ("MISTRAL_API_KEY", "ANTHROPIC_API_KEY", "AILU_USE_OLLAMA", "OPENAI_API_KEY"):
        os.environ.pop(key, None)
    os.environ["AILU_LLM_MOCK"] = "1"


def test_resolve_model_mistral_fast_picks_mistral_small():
    choice = ailu.resolve_model("fast", available=["mistral"])
    assert choice == {
        "provider": "mistral",
        "model": "mistral-small-latest",
        "recommended": True,
    }, choice


def test_resolve_model_anthropic_fast_picks_haiku():
    choice = ailu.resolve_model("fast", available=["anthropic"])
    assert choice["provider"] == "anthropic", choice
    assert choice["model"] == "claude-haiku-4-5", choice
    assert choice["recommended"] is True, choice


def test_resolve_model_override_wins_and_flags_not_recommended():
    choice = ailu.resolve_model(
        "frontier", available=["anthropic"], provider="mistral", model="mistral-tiny"
    )
    assert choice["provider"] == "mistral", choice
    assert choice["model"] == "mistral-tiny", choice
    assert choice["recommended"] is False, choice


def test_resolve_model_unknown_tier_raises():
    raised = False
    try:
        ailu.resolve_model("turbo", available=["mistral"])
    except ValueError:
        raised = True
    assert raised, "expected a ValueError on an unknown tier"


def test_available_providers_returns_list():
    providers = ailu.available_providers()
    assert isinstance(providers, list), providers
    assert all(isinstance(p, str) for p in providers), providers


def test_list_components_lists_the_catalog():
    components = ailu.list_components()
    assert isinstance(components, list)
    assert len(components) == len(set(components)), components
    for kind in ("promptBuilder", "bm25Retriever", "reranker", "councilAnonymize"):
        assert kind in components, components


def test_list_prebuilt_has_sixteen():
    agents = ailu.list_prebuilt()
    assert isinstance(agents, list)
    assert len(agents) == 16, [a.get("name") for a in agents]
    names = [a["name"] for a in agents]
    assert "summarizer" in names, names
    # camelCase wire shape from the Rust PrebuiltAgent.
    assert set(agents[0]).issuperset(
        {"name", "description", "tier", "systemPrompt", "toolNames", "outputChannel"}
    ), agents[0]


def test_run_component_prompt_builder_renders_template():
    update = ailu.run_component(
        "promptBuilder",
        {"template": "Hello {{name}}!", "into": "prompt"},
        {"name": "Ada"},
    )
    assert update == {"prompt": "Hello Ada!"}, update


def test_run_component_unknown_kind_raises_run_error():
    raised = False
    try:
        ailu.run_component("definitely-not-a-component", {}, {})
    except ailu.RunError:
        raised = True
    assert raised, "expected a RunError on an unknown component kind"


def test_run_prebuilt_summarizer_completes_on_mock():
    _force_mock_env()
    outcome = ailu.run_prebuilt("summarizer", "please summarise this text")
    assert outcome["status"] == "completed", outcome
    assert outcome["resolvedModel"]["provider"] == "mock", outcome
    # The summarizer writes into its `summary` output channel.
    assert "summary" in outcome["channels"], outcome["channels"]


def test_prebuilt_accessor_runs_named_agent_on_mock():
    _force_mock_env()
    outcome = ailu.prebuilt.summarizer("please summarise this text")
    assert outcome["status"] == "completed", outcome
    assert outcome["resolvedModel"]["provider"] == "mock", outcome


def test_run_prebuilt_without_a_key_names_the_variable_to_set():
    _force_mock_env()
    os.environ.pop("AILU_LLM_MOCK", None)
    raised = None
    try:
        ailu.run_prebuilt("summarizer", "please summarise this text")
    except ailu.RunError as error:
        raised = str(error)
    finally:
        os.environ["AILU_LLM_MOCK"] = "1"
    assert raised is not None and "AILU_LLM_MOCK=1" in raised, raised


def test_run_prebuilt_unknown_agent_raises_run_error():
    raised = False
    try:
        ailu.run_prebuilt("no-such-agent", "x")
    except ailu.RunError:
        raised = True
    assert raised, "expected a RunError on an unknown prebuilt agent"


# ---------------------------------------------------------------------------
# Graph runner (ADR 0045 D2.2) — host nodes, tools, conditions, events,
# cancellation, record and replay, on the same runner as the TypeScript SDK.
# ---------------------------------------------------------------------------

_CHANNELS = {
    "proposal": {"type": "string", "reducer": "replace"},
    "receipt": {"type": "string", "reducer": "replace"},
}


def _graph(nodes, edges, entry):
    return {
        "id": "g",
        "version": "0.0.0",
        "name": "g",
        "channels": dict(_CHANNELS),
        "nodes": [
            {"id": node_id, "type": node_type, "label": node_id} for node_id, node_type in nodes
        ],
        "edges": [{"id": f"e{i}", **edge} for i, edge in enumerate(edges)],
        "entryNodeId": entry,
    }


_SEND = _graph([("send", "action")], [], "send")
_GATED_SEND = _graph(
    [("review", "human-gate"), ("send", "action")],
    [{"from": "review", "to": "send", "type": "default"}],
    "review",
)


def _recording_send(receipt="r-1"):
    calls = []

    def send(node):
        calls.append(node)
        return {"receipt": receipt}

    return send, calls


def test_graph_runner_runs_a_host_node_with_its_channels_and_effect_key():
    send, calls = _recording_send()
    outcome = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send}).run({"proposal": "p-1"})
    assert outcome["status"] == "completed", outcome["status"]
    assert outcome["state"]["channels"]["receipt"] == "r-1"
    assert len(calls) == 1
    assert calls[0].node_id == "send"
    assert calls[0].channels["proposal"] == "p-1"
    assert len(calls[0].effect_key) == 64


def test_graph_runner_gives_a_retry_of_the_same_run_the_same_effect_key():
    send, calls = _recording_send()
    runner = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send})
    for run_id in ["run-1", "run-1", "run-2"]:
        runner.run({"proposal": "p-1"}, run_id=run_id)
    first, retry, other = (call.effect_key for call in calls)
    assert retry == first
    assert other != first


def test_graph_runner_acts_after_the_gate_once():
    send, calls = _recording_send()
    runner = ailu.GraphRunner({"graph": _GATED_SEND}, nodes={"send": send})
    paused = runner.run({"proposal": "p-1"})
    assert paused["status"] == "suspended"
    assert calls == []
    done = runner.resume(paused["state"])
    assert done["status"] == "completed"
    assert done["state"]["channels"]["receipt"] == "r-1"
    assert len(calls) == 1


def test_graph_runner_fails_the_run_when_a_step_raises():
    def send(node):
        raise ValueError("slack 503")

    outcome = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send}).run({"proposal": "p-1"})
    assert outcome["status"] == "failed"


def test_graph_runner_refuses_an_async_step():
    async def send(node):
        return {"receipt": "r-1"}

    outcome = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send}).run({"proposal": "p-1"})
    assert outcome["status"] == "failed"


def test_graph_runner_routes_with_a_python_condition_and_forwards_events():
    graph = _graph(
        [("check", "action"), ("send", "action"), ("skip", "action")],
        [
            {"from": "check", "to": "send", "type": "conditional", "condition": "approved"},
            {"from": "check", "to": "skip", "type": "default"},
        ],
        "check",
    )
    send, calls = _recording_send()
    events = []
    runner = ailu.GraphRunner(
        {"graph": graph, "hostNodeIds": ["check", "skip"]},
        nodes={"send": send},
        conditions={"approved": lambda channels: channels.get("proposal") == "p-1"},
        on_event=events.append,
    )
    assert runner.run({"proposal": "p-1"})["status"] == "completed"
    assert len(calls) == 1
    assert runner.run({"proposal": "p-2"})["status"] == "completed"
    assert len(calls) == 1  # p-2 took the default edge
    assert any(event.get("type") == "run_completed" for event in events), events


def test_graph_runner_fails_on_a_condition_it_was_not_given():
    graph = _graph(
        [("check", "action"), ("send", "action")],
        [{"from": "check", "to": "send", "type": "conditional", "condition": "approved"}],
        "check",
    )
    outcome = ailu.GraphRunner({"graph": graph, "hostNodeIds": ["check", "send"]}).run({})
    assert outcome["status"] == "failed"


def test_graph_runner_stops_at_a_node_boundary_when_cancelled():
    send, calls = _recording_send()
    outcome = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send}).run(
        {"proposal": "p-1"}, is_cancelled=lambda: True
    )
    assert outcome["status"] == "cancelled"
    assert calls == []


def test_graph_runner_delivers_a_signal_to_a_waiting_run():
    graph = _graph(
        [("wait", "action"), ("send", "action")],
        [{"from": "wait", "to": "send", "type": "default"}],
        "wait",
    )
    send, calls = _recording_send()
    runner = ailu.GraphRunner(
        {"graph": graph},
        nodes={"wait": lambda node: {"__waitForSignal": "paid"}, "send": send},
    )
    waiting = runner.run({"proposal": "p-1"})
    assert waiting["status"] == "suspended"
    done = runner.signal(waiting["state"], "paid", {"amount": 42})
    assert done["status"] == "completed"
    assert len(calls) == 1


def test_graph_runner_calls_a_host_tool_from_an_agent():
    graph = {
        "id": "g",
        "version": "0.0.0",
        "name": "g",
        "channels": {"answer": {"type": "agentResult", "reducer": "replace"}},
        "nodes": [{"id": "worker", "type": "agent", "label": "worker"}],
        "edges": [],
        "entryNodeId": "worker",
    }
    inputs = []

    def lookup(tool_input):
        inputs.append(tool_input)
        return {"hits": ["doc-1"]}

    runner = ailu.GraphRunner(
        {
            "graph": graph,
            "agents": {
                "worker": {"provider": "mock", "toolNames": ["lookup"], "outputChannel": "answer"}
            },
        },
        tools={"lookup": lookup},
    )
    assert runner.run({})["status"] == "completed"
    assert len(inputs) >= 1


def test_graph_runner_replay_serves_the_step_without_calling_it():
    send, calls = _recording_send()
    runner = ailu.GraphRunner({"graph": _SEND}, nodes={"send": send})
    saved = os.environ.get("AILU_LLM_RECORD")
    os.environ["AILU_LLM_RECORD"] = "1"
    try:
        recorded = runner.run({"proposal": "p-1"}, run_id="run-recorded")
    finally:
        if saved is None:
            del os.environ["AILU_LLM_RECORD"]
        else:
            os.environ["AILU_LLM_RECORD"] = saved
    assert recorded["status"] == "completed"
    assert len(calls) == 1
    replayed = runner.replay(recorded["entryState"], "audit-1", recorded["replayJournal"])
    assert replayed["status"] == "completed"
    assert replayed["state"]["channels"]["receipt"] == "r-1"
    assert len(calls) == 1, "a replay must never call a step"


def test_graph_runner_replay_that_diverges_raises_run_error():
    runner = ailu.GraphRunner({"graph": _SEND}, nodes={"send": _recording_send()[0]})
    entry = runner.run({"proposal": "p-1"}, run_id="run-x")["state"]
    entry = {**entry, "status": "running", "currentNodeId": "send", "version": 0}
    journal = json.dumps({"decisions": {"calls": []}, "clock": [], "nodeResults": []})
    raised = False
    try:
        runner.replay(entry, "audit-2", journal)
    except ailu.RunError as error:
        raised = "node_input_mismatch" in str(error)
    assert raised, "expected a RunError naming the divergence"


def test_graph_runner_refuses_a_spec_without_a_graph():
    raised = False
    try:
        ailu.GraphRunner({})
    except ValueError:
        raised = True
    assert raised


# ---------------------------------------------------------------------------
# Catalog spec (ADR 0045 D3.2) — the engine reads a catalog graph's carriers.
# ---------------------------------------------------------------------------

_CATALOG_GOLDEN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "crates",
    "runtime-bridge",
    "tests",
    "fixtures",
    "catalog_spec_golden.json",
)


def test_engine_spec_from_catalog_matches_every_golden_case():
    # The cases the TypeScript SDK recorded: Python reaches the same engine function.
    with open(_CATALOG_GOLDEN, encoding="utf-8") as golden_file:
        cases = json.load(golden_file)
    assert len(cases) >= 30
    for case in cases:
        out = json.loads(ailu._native.engine_spec_from_catalog(json.dumps(case["input"])))
        if "error" in out:
            error = out["error"]
            out = {
                "error": {key: error[key] for key in ("kind", "nodeId", "reason") if key in error}
            }
        assert out == case["expected"], case["name"]


def test_catalog_runner_drives_the_engine_spec_of_every_golden_case():
    # What run_catalog_graph sends the engine is the golden spec (GraphRunner sorts the ids).
    with open(_CATALOG_GOLDEN, encoding="utf-8") as golden_file:
        cases = json.load(golden_file)
    for case in cases:
        if "error" in case["expected"]:
            continue
        given = case["input"]
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", RuntimeWarning)  # the malformed-carrier cases warn
            runner = ailu._catalog_runner(
                given["graph"],
                nodes={node_id: _recording_send()[0] for node_id in given.get("hostNodes", [])},
                tools={name: (lambda tool_input: {}) for name in given.get("hostTools", [])},
                subgraphs=given.get("subgraphs"),
                provider_keys=given.get("providerKeys"),
                fs_policy=given.get("fsPolicy"),
                skills=given.get("skills"),
                on_event=None,
            )
        expected = dict(case["expected"]["spec"])
        expected["hostNodeIds"] = sorted(expected["hostNodeIds"])
        expected["jsToolNames"] = sorted(expected["jsToolNames"])
        assert runner._base == expected, case["name"]


def _catalog_graph(nodes, edges=(), entry=None):
    return {
        "id": "saved",
        "version": "1",
        "name": "saved",
        "channels": {
            "name": {"type": "string", "reducer": "replace"},
            "prompt": {"type": "string", "reducer": "replace"},
            "answer": {"type": "agentResult", "reducer": "replace"},
            "receipt": {"type": "string", "reducer": "replace"},
        },
        "nodes": nodes,
        "edges": [{"id": f"e{i}", **edge} for i, edge in enumerate(edges)],
        "entryNodeId": entry or nodes[0]["id"],
    }


_PROMPT = {
    "id": "prompt",
    "type": "action",
    "label": "prompt",
    "metadata": {
        "component": {
            "kind": "promptBuilder",
            "params": {"template": "Hi {{name}}", "into": "prompt"},
        }
    },
}
_ASSISTANT = {
    "id": "assistant",
    "type": "agent",
    "label": "assistant",
    "metadata": {"agent": {"system": "Answer.", "outputChannel": "answer"}},
}
_SEND_NODE = {"id": "send", "type": "action", "label": "send"}
_REVIEW = {"id": "review", "type": "human-gate", "label": "review"}


def test_run_catalog_graph_runs_components_agents_and_a_bound_step():
    _force_mock_env()
    send, calls = _recording_send()
    graph = _catalog_graph(
        [_PROMPT, _ASSISTANT, _SEND_NODE],
        [
            {"from": "prompt", "to": "assistant", "type": "default"},
            {"from": "assistant", "to": "send", "type": "default"},
        ],
    )
    outcome = ailu.run_catalog_graph(graph, initial_data={"name": "Ada"}, nodes={"send": send})
    assert outcome["status"] == "completed", outcome
    channels = outcome["state"]["channels"]
    assert channels["prompt"] == "Hi Ada"
    assert channels["answer"]["reasoning"] is not None
    assert channels["receipt"] == "r-1"
    assert len(calls) == 1 and len(calls[0].effect_key) == 64
    assert calls[0].channels["prompt"] == "Hi Ada"


def test_resume_catalog_graph_acts_after_the_gate_once():
    send, calls = _recording_send()
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(graph, nodes={"send": send})
    assert paused["status"] == "suspended" and calls == []
    done = ailu.resume_catalog_graph(graph, paused["state"], nodes={"send": send})
    assert done["status"] == "completed"
    assert done["state"]["channels"]["receipt"] == "r-1"
    assert len(calls) == 1



class _KeepingStore:
    """A checkpoint store that keeps every checkpoint it is given (ADR 0049 D1)."""

    def __init__(self):
        self.kept = []

    def save(self, checkpoint):
        self.kept.append(checkpoint)


class _DownStore:
    """A checkpoint store that cannot keep anything."""

    def save(self, checkpoint):
        raise OSError("store down")


def test_a_checkpointer_keeps_every_checkpoint_the_last_being_the_outcomes():
    send, _calls = _recording_send()
    store = _KeepingStore()
    graph = _catalog_graph([_SEND_NODE], [])
    outcome = ailu.run_catalog_graph(graph, nodes={"send": send}, checkpointer=store)
    assert outcome["status"] == "completed", outcome
    statuses = [checkpoint["graphState"]["status"] for checkpoint in store.kept]
    assert statuses == ["running", "completed"], statuses
    assert store.kept[-1]["id"] == outcome["state"]["checkpointId"]
    assert store.kept[-1]["graphState"]["channels"]["receipt"] == "r-1"


def test_a_store_that_cannot_keep_a_checkpoint_stops_the_run_before_its_step():
    send, calls = _recording_send()
    graph = _catalog_graph([_SEND_NODE], [])
    try:
        ailu.run_catalog_graph(graph, nodes={"send": send}, checkpointer=_DownStore())
        raise AssertionError("expected the run to stop")
    except ailu.RunError as error:
        assert "store down" in str(error), str(error)
    assert calls == []


def test_a_resumed_run_hands_its_checkpoints_to_the_store_too():
    send, _calls = _recording_send()
    store = _KeepingStore()
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(graph, nodes={"send": send}, checkpointer=store)
    assert paused["status"] == "suspended"
    assert store.kept[-1]["graphState"]["status"] == "suspended"
    before = len(store.kept)
    done = ailu.resume_catalog_graph(
        graph, paused["state"], nodes={"send": send}, checkpointer=store
    )
    assert done["status"] == "completed"
    assert len(store.kept) > before
    assert store.kept[-1]["graphState"]["status"] == "completed"


def test_a_run_resumed_from_a_running_checkpoint_runs_its_current_step_again_only():
    # ADR 0049 D3: the checkpoint written after `first` resumes by running `second` again.
    ran = []

    def step(name):
        def run_step(_input):
            ran.append(name)
            return {}

        return run_step

    nodes = {"first": step("first"), "second": step("second")}
    graph = _catalog_graph(
        [
            {"id": "first", "type": "action", "label": "first"},
            {"id": "second", "type": "action", "label": "second"},
        ],
        [{"from": "first", "to": "second", "type": "default"}],
    )
    store = _KeepingStore()
    ailu.run_catalog_graph(graph, nodes=nodes, checkpointer=store)
    after_first = next(
        checkpoint
        for checkpoint in store.kept
        if checkpoint["graphState"]["status"] == "running"
        and checkpoint["graphState"]["currentNodeId"] == "second"
    )
    ran.clear()
    recovered = ailu.resume_catalog_graph(graph, after_first["graphState"], nodes=nodes)
    assert recovered["status"] == "completed", recovered
    assert ran == ["second"], ran

def test_run_catalog_graph_runs_a_step_of_a_subgraph():
    send, calls = _recording_send()
    child = _catalog_graph([_SEND_NODE])
    child["id"] = "child"
    parent = _catalog_graph(
        [{"id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child"}]
    )
    outcome = ailu.run_catalog_graph(parent, subgraphs=[child], nodes={"send": send})
    assert outcome["status"] == "completed", outcome
    assert len(calls) == 1


def test_run_catalog_graph_calls_a_tool_an_agent_carrier_names():
    _force_mock_env()
    inputs = []
    agent = {
        **_ASSISTANT,
        "metadata": {"agent": {"toolNames": ["lookup"], "outputChannel": "answer"}},
    }
    outcome = ailu.run_catalog_graph(
        _catalog_graph([agent]),
        tools={"lookup": lambda tool_input: inputs.append(tool_input) or {"hits": 1}},
    )
    assert outcome["status"] == "completed", outcome
    assert len(inputs) == 1


def test_run_catalog_graph_refuses_a_binding_to_a_node_the_engine_runs():
    graph = _catalog_graph([_ASSISTANT, _SEND_NODE])
    for node_id, reason in [
        ("assistant", "it is not a plain step"),
        ("nope", "the graph and its subgraphs have no node with this id"),
    ]:
        try:
            ailu.run_catalog_graph(graph, nodes={node_id: _recording_send()[0]})
            raise AssertionError(f"binding {node_id!r} should be refused")
        except ailu.HostNodeBindingError as error:
            assert error.node_id == node_id
            assert error.reason.startswith(reason), error.reason
            assert isinstance(error, ailu.RunError)


def test_run_catalog_graph_refuses_a_carrier_it_cannot_read():
    agent = {**_ASSISTANT, "metadata": {"agent": {"toolSpecs": "lookup"}}}
    raised = None
    try:
        ailu.run_catalog_graph(_catalog_graph([agent]))
    except ailu.RunError as error:
        raised = str(error)
    assert raised is not None and "assistant" in raised, raised


def test_run_catalog_graph_warns_on_a_malformed_map_agents_carrier():
    fan = {
        "id": "fan",
        "type": "action",
        "label": "fan",
        "metadata": {"mapAgents": {"joinAt": "x"}},
    }
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        outcome = ailu.run_catalog_graph(_catalog_graph([fan]))
    assert outcome["status"] == "completed"
    assert [str(warning.message) for warning in caught] == [
        'node "fan" has a malformed mapAgents carrier (needs overChannel, joinAt, subAgent)'
        " — it will NOT fan out."
    ]


def test_run_catalog_graph_does_not_take_a_conditional_edge():
    send, calls = _recording_send()
    first = {"id": "first", "type": "action", "label": "first"}
    graph = _catalog_graph(
        [first, _SEND_NODE],
        [{"from": "first", "to": "send", "type": "conditional", "condition": "go"}],
    )
    outcome = ailu.run_catalog_graph(graph, nodes={"send": send})
    assert outcome["status"] == "completed", outcome
    assert calls == []


def test_an_agent_narrowed_to_other_channels_is_not_shown_the_brain():
    # ADR 0047: `visibleChannels` bounds the brain like the rest of the state. The recorded request
    # is what the provider would have received.
    _force_mock_env()

    def first_message(visible):
        agent = {
            **_ASSISTANT,
            "metadata": {
                "agent": {
                    "system": "Answer.",
                    "outputChannel": "answer",
                    **({"visibleChannels": visible} if visible is not None else {}),
                }
            },
        }
        saved = os.environ.get("AILU_LLM_RECORD")
        os.environ["AILU_LLM_RECORD"] = "1"
        try:
            outcome = ailu.run_catalog_graph(
                _catalog_graph([agent]),
                initial_data={"name": "Ada", "__brainRecall": ["Acme — a customer since 2019"]},
            )
        finally:
            if saved is None:
                del os.environ["AILU_LLM_RECORD"]
            else:
                os.environ["AILU_LLM_RECORD"] = saved
        journal = json.loads(outcome["replayJournal"])
        return journal["decisions"]["calls"][0]["request"]["messages"][0]["content"]

    assert "Governed knowledge" in first_message(None)
    assert "Governed knowledge" in first_message(["name", "__brainRecall"])
    narrowed = first_message(["name"])
    assert "Governed knowledge" not in narrowed and "Acme" not in narrowed


def test_replay_catalog_graph_serves_the_recorded_run():
    _force_mock_env()
    send, calls = _recording_send()
    graph = _catalog_graph(
        [_ASSISTANT, _SEND_NODE], [{"from": "assistant", "to": "send", "type": "default"}]
    )
    saved = os.environ.get("AILU_LLM_RECORD")
    os.environ["AILU_LLM_RECORD"] = "1"
    try:
        recorded = ailu.run_catalog_graph(graph, run_id="run-saved", nodes={"send": send})
    finally:
        if saved is None:
            del os.environ["AILU_LLM_RECORD"]
        else:
            os.environ["AILU_LLM_RECORD"] = saved
    assert recorded["status"] == "completed" and len(calls) == 1
    replayed = ailu.replay_catalog_graph(
        graph, recorded["entryState"], "audit-1", recorded["replayJournal"]
    )
    assert replayed["status"] == "completed"
    assert replayed["state"]["channels"]["receipt"] == "r-1"
    assert replayed["state"]["channels"]["answer"] == recorded["state"]["channels"]["answer"]
    assert len(calls) == 1, "a replay must never call a step"


# ---------------------------------------------------------------------------
# Catalog approvals (ADR 0045 D3.1) — what a run files, whether a resume may go on.
# ---------------------------------------------------------------------------

_APPROVALS_GOLDEN = os.path.join(os.path.dirname(_CATALOG_GOLDEN), "catalog_approvals_golden.json")


def test_engine_catalog_approval_decisions_match_every_golden_case():
    # The decisions the TypeScript SDK recorded: Python reaches the same engine functions.
    with open(_APPROVALS_GOLDEN, encoding="utf-8") as golden_file:
        cases = json.load(golden_file)
    assert len(cases) >= 30
    for case in cases:
        given = case["input"]
        if case["kind"] == "filing":
            plan = json.loads(ailu._native.engine_catalog_approval_plan(json.dumps(given)))
            kept = (
                [] if plan["clearApprovalIds"] else given["state"]["channels"].get("__approvalIds")
            )
            filed = [f"filed-{n}" for n in range(len(plan["requests"]))]
            got = {"requests": plan["requests"], "approvalIds": filed or kept}
            # ADR 0045 rev. 1 R6: a refused plan says why; every other case keeps its shape.
            if "refusal" in plan:
                got["refusal"] = plan["refusal"]
        else:
            got = {
                "reads": json.loads(
                    ailu._native.engine_catalog_approvals_to_check(json.dumps(given["state"]))
                ),
                "problems": json.loads(
                    ailu._native.engine_catalog_resume_problems(json.dumps(given))
                ),
            }
        assert got == case["expected"], case["name"]


def _refused(call):
    try:
        call()
    except ailu.ApprovalNotGrantedError as error:
        return error.problems
    raise AssertionError("expected ApprovalNotGrantedError")


def test_a_governed_gate_is_filed_and_the_resume_waits_for_its_approval():
    engine = ailu.InMemoryApprovalEngine()
    send, calls = _recording_send()
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(
        graph, run_id="run-gov", nodes={"send": send}, approval_engine=engine
    )
    assert paused["status"] == "suspended"
    [pending] = engine.get_pending("run-gov")
    assert pending["subject"] == {"description": "gate:review"}
    assert pending["requested_by"] == "review"
    assert paused["state"]["channels"]["__approvalIds"] == [pending["id"]]

    resume = lambda: ailu.resume_catalog_graph(  # noqa: E731
        graph, paused["state"], nodes={"send": send}, approval_engine=engine
    )
    assert _refused(resume) == [f"request {pending['id']} (gate:review) is still pending"]
    assert calls == [], "nothing runs before the approval"
    engine.approve(pending["id"], "alice")
    done = resume()
    assert done["status"] == "completed"
    assert len(calls) == 1
    assert engine.get_pending("run-gov") == []


def test_a_rejected_gate_refuses_the_resume():
    engine = ailu.InMemoryApprovalEngine()
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(graph, approval_engine=engine)
    [pending] = engine.get_pending()
    engine.reject(pending["id"], "alice", "not this week")
    problems = _refused(
        lambda: ailu.resume_catalog_graph(graph, paused["state"], approval_engine=engine)
    )
    assert problems == [f"request {pending['id']} (gate:review) was rejected by alice"]


def test_a_gated_tool_needs_the_grant_of_the_person_who_approved_it():
    _force_mock_env()
    engine = ailu.InMemoryApprovalEngine()
    refunds = []
    assistant = {
        **_ASSISTANT,
        "metadata": {
            "agent": {
                "toolNames": ["refund"],
                "approvalToolNames": ["refund"],
                "suspendForApproval": True,
                "outputChannel": "answer",
            }
        },
    }
    graph = _catalog_graph([assistant])
    tools = {"refund": lambda tool_input: refunds.append(tool_input) or {"ok": True}}
    paused = ailu.run_catalog_graph(graph, tools=tools, approval_engine=engine)
    assert paused["status"] == "suspended" and refunds == []
    [pending] = engine.get_pending(paused["state"]["runId"])
    assert pending["subject"] == {"description": "tool:refund"}
    assert pending["requested_by"] == "assistant"
    engine.approve(pending["id"], "alice")

    def resume(approver):
        grant = [{"name": "refund", "requestedBy": "assistant", "resolvedBy": approver}]
        return ailu.resume_catalog_graph(
            graph, paused["state"], tools=tools, approved_tools=grant, approval_engine=engine
        )

    assert _refused(lambda: resume("mallory")) == [
        "tool 'refund' has no request approved by 'mallory' in the approval engine"
    ]
    assert resume("alice")["status"] == "completed"
    assert len(refunds) == 1


def test_a_conditioned_tool_is_gated_per_call_and_its_grant_is_that_call():
    # ADR 0046: the mock calls `refund` with `{}` — `amount` is missing, so the gate opens
    # (fail-closed), files the call's key, input and what crossed; a name grant unlocks
    # nothing, the call's key unlocks that call.
    _force_mock_env()
    engine = ailu.InMemoryApprovalEngine()
    refunds = []
    assistant = {
        **_ASSISTANT,
        "metadata": {
            "agent": {
                "toolNames": ["refund"],
                "approvalToolNames": ["refund"],
                "approvalWhen": {"refund": [{"argument": "amount", "above": 500}]},
                "suspendForApproval": True,
                "outputChannel": "answer",
            }
        },
    }
    graph = _catalog_graph([assistant])
    tools = {"refund": lambda tool_input: refunds.append(tool_input) or {"ok": True}}
    paused = ailu.run_catalog_graph(graph, tools=tools, approval_engine=engine)
    assert paused["status"] == "suspended" and refunds == []
    [pending] = engine.get_pending(paused["state"]["runId"])
    subject = pending["subject"]
    assert subject["description"] == "tool:refund"
    assert subject["condition"] == "amount missing"
    assert subject["input"] == {}
    assert subject["approvalKey"].startswith("refund#") and len(subject["approvalKey"]) == 71
    engine.approve(pending["id"], "alice")

    def resume(grant):
        return ailu.resume_catalog_graph(
            graph, paused["state"], tools=tools, approved_tools=[grant], approval_engine=engine
        )

    named = {"name": "refund", "requestedBy": "assistant", "resolvedBy": "alice"}
    assert resume(named)["status"] == "suspended"
    assert refunds == []
    keyed = {**named, "key": subject["approvalKey"]}
    assert resume(keyed)["status"] == "completed"
    assert len(refunds) == 1


def test_the_builder_carries_a_tools_approval_conditions():
    refund = ailu.Tool(
        lambda tool_input: {},
        requires_approval=True,
        approval_when=[{"argument": "amount", "above": 500}],
    )
    graph = (
        ailu.create_graph("Refunds")
        .agent_node("assistant", system="Help.", tools={"refund": refund})
        .compile()
        .definition
    )
    [agent] = [node for node in graph["nodes"] if node["id"] == "assistant"]
    carrier = agent["metadata"]["agent"]
    assert carrier["approvalToolNames"] == ["refund"]
    assert carrier["approvalWhen"] == {"refund": [{"argument": "amount", "above": 500}]}


def test_the_builder_carries_named_values():
    # ADR 0048: `in` passes through as written, next to a threshold.
    delegate = ailu.Tool(
        lambda tool_input: {},
        requires_approval=True,
        approval_when=[
            {"argument": "agentName", "in": ("Nordlys", "Veritas")},
            {"argument": "budget", "above": 1000},
        ],
    )
    graph = (
        ailu.create_graph("Delegations")
        .agent_node("assistant", system="Help.", tools={"a2a_delegate": delegate})
        .compile()
        .definition
    )
    [agent] = [node for node in graph["nodes"] if node["id"] == "assistant"]
    assert agent["metadata"]["agent"]["approvalWhen"] == {
        "a2a_delegate": [
            {"argument": "agentName", "in": ["Nordlys", "Veritas"]},
            {"argument": "budget", "above": 1000},
        ]
    }


def test_a_named_value_condition_gates_on_the_catalog_path():
    # ADR 0048: the mock calls `a2a_delegate` with `{}` — `agentName` is missing, so the
    # gate opens (fail-closed) and files what crossed; the call's key unlocks that call.
    _force_mock_env()
    engine = ailu.InMemoryApprovalEngine()
    delegated = []
    assistant = {
        **_ASSISTANT,
        "metadata": {
            "agent": {
                "toolNames": ["a2a_delegate"],
                "approvalToolNames": ["a2a_delegate"],
                "approvalWhen": {
                    "a2a_delegate": [{"argument": "agentName", "in": ["Nordlys"]}]
                },
                "suspendForApproval": True,
                "outputChannel": "answer",
            }
        },
    }
    graph = _catalog_graph([assistant])
    tools = {"a2a_delegate": lambda tool_input: delegated.append(tool_input) or {"ok": True}}
    paused = ailu.run_catalog_graph(graph, tools=tools, approval_engine=engine)
    assert paused["status"] == "suspended" and delegated == []
    [pending] = engine.get_pending(paused["state"]["runId"])
    subject = pending["subject"]
    assert subject["description"] == "tool:a2a_delegate"
    assert subject["condition"] == "agentName missing"
    assert subject["approvalKey"].startswith("a2a_delegate#")
    engine.approve(pending["id"], "alice")
    grant = {
        "name": "a2a_delegate",
        "requestedBy": "assistant",
        "resolvedBy": "alice",
        "key": subject["approvalKey"],
    }
    done = ailu.resume_catalog_graph(
        graph, paused["state"], tools=tools, approved_tools=[grant], approval_engine=engine
    )
    assert done["status"] == "completed"
    assert len(delegated) == 1


def test_a_named_value_condition_without_a_value_is_refused():
    # ADR 0048: refused when the agent is built, never ignored.
    _force_mock_env()
    assistant = {
        **_ASSISTANT,
        "metadata": {
            "agent": {
                "toolNames": ["a2a_delegate"],
                "approvalToolNames": ["a2a_delegate"],
                "approvalWhen": {"a2a_delegate": [{"argument": "agentName", "in": []}]},
                "outputChannel": "answer",
            }
        },
    }
    graph = _catalog_graph([assistant])
    try:
        ailu.run_catalog_graph(graph, tools={"a2a_delegate": lambda tool_input: {}})
        raise AssertionError("expected the agent to be refused")
    except ailu.RunError as error:
        assert "names no value" in str(error), str(error)


def test_a_run_started_without_the_engine_cannot_resume_with_it():
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(graph)
    assert "__approvalIds" not in paused["state"]["channels"]
    problems = _refused(
        lambda: ailu.resume_catalog_graph(
            graph, paused["state"], approval_engine=ailu.InMemoryApprovalEngine()
        )
    )
    assert problems == [
        "the run waits on an approval that was never recorded: start it with the same approvalEngine"
    ]


def test_a_child_runs_gate_is_filed_under_the_child_run():
    engine = ailu.InMemoryApprovalEngine()
    child = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    child["id"] = "child"
    parent = _catalog_graph(
        [{"id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child"}]
    )
    paused = ailu.run_catalog_graph(
        parent, run_id="run-parent", subgraphs=[child], approval_engine=engine
    )
    assert paused["status"] == "suspended"
    assert engine.get_pending("run-parent") == []
    [pending] = engine.get_pending("run-parent:sub")
    assert pending["requested_by"] == "run-parent:sub:review"
    assert pending["subject"] == {"description": "gate:run-parent:sub:review"}


def test_a_tool_approval_in_a_child_run_is_refused_not_looped():
    # ADR 0045 rev. 1 R6: no grant reaches a child run yet, so an approved child call would be
    # asked for again on every resume. A governed resume of such a wait is refused, before the
    # approval engine is read and before anything runs.
    child = _catalog_graph(
        [
            {
                "id": "c_agent",
                "type": "agent",
                "label": "c_agent",
                "metadata": {"agent": {"toolNames": ["refund"], "suspendForApproval": True}},
            }
        ]
    )
    child["id"] = "child"
    parent = _catalog_graph(
        [{"id": "sub", "type": "subgraph", "label": "sub", "subgraphId": "child"}]
    )
    waiting = {
        "runId": "run-1",
        "graphId": "saved",
        "currentNodeId": "sub",
        "status": "suspended",
        "version": 1,
        "createdAt": "0",
        "updatedAt": "0",
        "channels": {
            "__subgraphStates": {
                "run-1:sub": {
                    "runId": "run-1:sub",
                    "graphId": "child",
                    "currentNodeId": "c_agent",
                    "status": "suspended",
                    "version": 1,
                    "createdAt": "0",
                    "updatedAt": "0",
                    "channels": {
                        "agentResult": {"approvalRequests": [{"subject": "tool:refund"}]}
                    },
                }
            }
        },
    }
    engine = ailu.InMemoryApprovalEngine()
    try:
        ailu.resume_catalog_graph(parent, waiting, subgraphs=[child], approval_engine=engine)
        raise AssertionError("a wait no person can decide was resumed")
    except ailu.ApprovalRefusedError as error:
        assert error.reason == (
            "a tool approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6)"
        )
        assert error.state["status"] == "suspended"
    assert engine.get_pending("run-1:sub") == []


def test_the_in_memory_engine_refuses_self_approval_and_the_runner_does_not_trust_a_store_that_allows_it():
    engine = ailu.InMemoryApprovalEngine()
    request = engine.request(
        run_id="r", node_id="review", requested_by="review", subject={"description": "gate:review"}
    )
    try:
        engine.approve(request["id"], "review")
        raise AssertionError("a requester approved its own request")
    except ailu.ApprovalSelfApprovalError:
        pass

    class Permissive(ailu.InMemoryApprovalEngine):
        def get_by_id(self, request_id):
            stored = super().get_by_id(request_id)
            return stored and {
                **stored,
                "status": "approved",
                "resolved_by": stored["requested_by"],
            }

    permissive = Permissive()
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    paused = ailu.run_catalog_graph(graph, approval_engine=permissive)
    [pending] = permissive.get_pending()
    problems = _refused(
        lambda: ailu.resume_catalog_graph(graph, paused["state"], approval_engine=permissive)
    )
    assert problems == [f"request {pending['id']} (gate:review) was approved by its own requester"]


def test_a_resume_that_moves_on_drops_the_ids_stashed_for_the_previous_wait():
    # As in TypeScript, with or without an approval engine.
    first = {"id": "legal", "type": "human-gate", "label": "legal"}
    second = {"id": "finance", "type": "human-gate", "label": "finance"}
    graph = _catalog_graph([first, second], [{"from": "legal", "to": "finance", "type": "default"}])
    paused = ailu.run_catalog_graph(graph)
    stale = {
        **paused["state"],
        "channels": {**paused["state"]["channels"], "__approvalIds": ["legal-1"]},
    }
    moved = ailu.resume_catalog_graph(graph, stale)
    assert moved["state"]["currentNodeId"] == "finance"
    assert moved["state"]["channels"]["__approvalIds"] == []


# ---------------------------------------------------------------------------
# Reading a run (ADR 0045 D3.4) — explain a run, check a replay.
# ---------------------------------------------------------------------------

_INSIGHT_GOLDEN = os.path.join(os.path.dirname(_CATALOG_GOLDEN), "run_insight_golden.json")


def test_explain_run_and_verify_replay_decisions_match_every_golden_case():
    # The answers the TypeScript SDK recorded: Python gets them from the same engine functions.
    with open(_INSIGHT_GOLDEN, encoding="utf-8") as golden_file:
        cases = json.load(golden_file)
    assert len(cases) >= 20
    for case in cases:
        given = case["input"]
        if case["kind"] == "explain":
            got = ailu.explain_run(given["state"], given.get("events"))
        else:
            got = ailu.verify_replay_decisions(given["attested"], given["replayed"])
        assert got == case["expected"], case["name"]


def test_explain_run_reads_a_suspended_catalog_run():
    graph = _catalog_graph(
        [_REVIEW, _SEND_NODE], [{"from": "review", "to": "send", "type": "default"}]
    )
    events = []
    paused = ailu.run_catalog_graph(graph, on_event=events.append)
    explained = ailu.explain_run(paused["state"], events)
    assert explained["status"] == "suspended"
    assert explained["suspended"]["node"] == "review"
    assert explained["recentEvents"][-1]["type"] == "run_suspended"


# ---------------------------------------------------------------------------
# Durable timers and signals, token streaming (ADR 0045 M4).
# ---------------------------------------------------------------------------


def test_a_step_that_sleeps_suspends_the_run_until_it_is_resumed():
    calls = []

    def wait(node):
        calls.append(node.node_id)
        return ailu.sleep_until("2026-10-03T08:00:00Z", {"receipt": "scheduled"})

    graph = _graph(
        [("wait", "action"), ("send", "action")],
        [{"from": "wait", "to": "send", "type": "default"}],
        "wait",
    )
    send, sent = _recording_send("r-after-timer")
    runner = ailu.GraphRunner({"graph": graph}, nodes={"wait": wait, "send": send})
    paused = runner.run()
    assert paused["status"] == "suspended"
    assert paused["state"]["channels"]["receipt"] == "scheduled"
    assert ailu.read_suspend_meta(paused["state"]) == {
        "reason": "timer",
        "wakeAt": "2026-10-03T08:00:00Z",
    }
    done = runner.resume(paused["state"])
    assert done["status"] == "completed"
    assert calls == ["wait"], "a resumed timer continues past its step"
    assert len(sent) == 1
    assert ailu.read_suspend_meta(done["state"]) is None


def test_a_step_that_waits_for_a_signal_reads_its_payload_after_delivery():
    seen = []

    def ask(node):
        return ailu.wait_for_signal("paid", wake_at="2026-10-10T00:00:00Z")

    def record(node):
        seen.append(ailu.read_signal(node, "paid"))
        return {}

    graph = _graph(
        [("ask", "action"), ("record", "action")],
        [{"from": "ask", "to": "record", "type": "default"}],
        "ask",
    )
    runner = ailu.GraphRunner({"graph": graph}, nodes={"ask": ask, "record": record})
    paused = runner.run()
    meta = ailu.read_suspend_meta(paused["state"])
    assert meta["reason"] == "signal" and meta["awaitingSignal"] == "paid"
    assert meta["wakeAt"] == "2026-10-10T00:00:00Z"
    done = runner.signal(paused["state"], "paid", {"amount": 42})
    assert done["status"] == "completed"
    assert seen == [{"amount": 42}]
    assert ailu.read_signal(done["state"], "paid") == {"amount": 42}
    assert ailu.read_signal(done["state"], "other") is None


def test_stream_tokens_streams_an_agent_reply_as_token_delta_events():
    _force_mock_env()

    def deltas(stream_tokens):
        events = []
        outcome = ailu.run_catalog_graph(
            _catalog_graph([_ASSISTANT]), on_event=events.append, stream_tokens=stream_tokens
        )
        assert outcome["status"] == "completed"
        return [event for event in events if event["type"] == "token_delta"]

    streamed = deltas(True)
    assert len(streamed) >= 1
    assert streamed[0]["nodeId"] == "assistant" and streamed[0]["delta"]
    assert deltas(False) == []


def _fan_graph():
    graph = _catalog_graph(
        [
            {
                "id": "fan",
                "type": "action",
                "label": "fan",
                "metadata": {
                    "mapAgents": {
                        "overChannel": "items",
                        "joinAt": "reports",
                        "subAgent": {"system": "Summarise."},
                    }
                },
            }
        ]
    )
    graph["channels"]["items"] = {"type": "json", "reducer": "replace"}
    graph["channels"]["reports"] = {"type": "json", "reducer": "replace"}
    return graph


def test_map_agents_reports_each_spawn_started_then_completed():
    # ADR 0050: one spawn_started (with its item) and one spawn_completed per item.
    _force_mock_env()
    events = []
    outcome = ailu.run_catalog_graph(
        _fan_graph(), initial_data={"items": ["a", "b"]}, on_event=events.append
    )
    assert outcome["status"] == "completed"
    started = [e for e in events if e["type"] == "spawn_started"]
    completed = [e for e in events if e["type"] == "spawn_completed"]
    assert sorted((e["spawnId"], e["itemIndex"], e["item"]) for e in started) == [
        (0, 0, "a"),
        (1, 1, "b"),
    ]
    assert sorted(e["spawnId"] for e in completed) == [0, 1]
    for event in completed:
        assert event["nodeId"] == "fan"
        assert event["output"] == outcome["state"]["channels"]["reports"][event["spawnId"]]
        start = next(i for i, e in enumerate(events) if e in started and e["spawnId"] == event["spawnId"])
        assert start < events.index(event)


def test_map_agents_over_a_non_list_fails_the_node_clearly():
    # ADR 0050 (ailu#2033): text where a list is expected fails, instead of running nothing.
    _force_mock_env()
    events = []
    outcome = ailu.run_catalog_graph(
        _fan_graph(), initial_data={"items": '["a", "b"]'}, on_event=events.append
    )
    assert outcome["status"] == "failed"
    failed = [e for e in events if e["type"] == "node_failed"]
    assert failed[0]["error"] == (
        "mapAgents node 'fan': overChannel 'items' must be a JSON array, got string"
    )
    assert not [e for e in events if e["type"].startswith("spawn_")]


# ---------------------------------------------------------------------------
# One model call (ADR 0045 M4) — the TypeScript model.invoke().
# ---------------------------------------------------------------------------


def test_llm_complete_answers_from_the_offline_mock():
    _force_mock_env()
    reply = ailu.llm_complete("Say hi.", provider="anthropic", max_tokens=32, temperature=0)
    assert reply["provider"] == "anthropic"
    assert reply["content"]
    by_tier = ailu.llm_complete(
        [{"role": "system", "content": "Be brief."}, {"role": "user", "content": "Hi"}],
        tier="fast",
    )
    assert by_tier["content"]


def test_llm_complete_needs_a_provider_or_a_tier_and_refuses_unknown_ones():
    for call, error in [
        (lambda: ailu.llm_complete("hi"), ValueError),
        (lambda: ailu.llm_complete("hi", provider="nope"), ailu.RunError),
        (
            lambda: ailu.llm_complete(
                "hi", base_url="http://localhost:1/v1", api_key_env="AILU_TEST_UNSET_KEY"
            ),
            ailu.RunError,
        ),
    ]:
        try:
            call()
            raise AssertionError("expected an error")
        except error:
            pass


def test_llm_complete_without_a_key_names_the_variable_to_set():
    saved = os.environ.pop("AILU_LLM_MOCK", None)
    key = os.environ.pop("MISTRAL_API_KEY", None)
    try:
        ailu.llm_complete("hi", provider="mistral")
        raise AssertionError("expected a missing-key error")
    except ailu.RunError as error:
        assert "MISTRAL_API_KEY" in str(error), str(error)
    finally:
        if saved is not None:
            os.environ["AILU_LLM_MOCK"] = saved
        if key is not None:
            os.environ["MISTRAL_API_KEY"] = key


# ---------------------------------------------------------------------------
# The graph builder (ADR 0045 M4) — the TypeScript createGraph.
# ---------------------------------------------------------------------------

_BUILDER_GOLDEN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "fixtures", "builder_golden.json"
)


def _step(node):
    return {}


def _golden_builders():
    lookup = ailu.Tool(
        lambda tool_input: {},
        description="Looks an order up.",
        input_schema={"type": "object", "properties": {"order": {"type": "string"}}},
    )
    refund = ailu.Tool(
        lambda tool_input: {}, description="Refunds an order.", requires_approval=True
    )
    support = (
        ailu.create_graph("Support Flow")
        .channel("ticket", "string", default="")
        .channel("prompt", "string")
        .node("prepare", _step)
        .component("build", "promptBuilder", {"template": "Ticket: {{ticket}}", "into": "prompt"})
        .agent_node(
            "assistant",
            system="Help.",
            tools={"lookup": lookup, "refund": refund},
            suspend_for_approval=True,
            max_iterations=3,
        )
        .agent_node("fast", system="Fast.", tier="fast", output_channel="quick")
        .human_gate("review")
        .edge("prepare", "build")
        .edge("build", "assistant")
        .conditional_edge("assistant", "review", "needs_review", lambda channels: True)
        .edge("review", "fast")
    )
    child = (
        ailu.create_graph("Child Task")
        .channel("in", "string", default="")
        .channel("out", "string")
        .node("child_step", _step)
    )
    nested = (
        ailu.create_graph("Nested", id="nested-graph", version="2.0.0")
        .channel("ticket", "string", default="")
        .channel("result", "string")
        .channel("secret", "string", no_log=True)
        .node("risky", _step, retry_policy={"maxAttempts": 3, "backoffMs": 10})
        .node("fallback", _step)
        .agent_node(
            "terse",
            system="Short.",
            output_style="terse",
            context_budget=4000,
            visible_channels=["ticket"],
            output_channel="summary",
        )
        .agent_node(
            "auto", system="Any provider.", provider="", tier="fast", output_channel="auto_out"
        )
        .subgraph("sub", child, input_mapping={"in": "ticket"}, output_mapping={"result": "out"})
        .error_edge("risky", "fallback")
        .edge("risky", "terse")
        .edge("terse", "auto")
        .edge("auto", "sub")
        .entry("risky")
    )
    return {"support": support, "nested": nested, "child": child}


def test_the_builder_writes_the_definitions_the_typescript_builder_writes():
    with open(_BUILDER_GOLDEN, encoding="utf-8") as golden_file:
        golden = json.load(golden_file)
    built = {name: builder.compile() for name, builder in _golden_builders().items()}
    for name, compiled in built.items():
        assert compiled.definition == golden[name], name
    assert built["nested"].subgraphs == [golden["child"]]


def test_a_built_graph_runs_its_steps_tools_conditions_and_gate():
    _force_mock_env()
    calls = []
    lookups = []

    def prepare(node):
        calls.append(("prepare", node.channels["ticket"]))
        return {"ticket": node.channels["ticket"].upper()}

    def file(node):
        calls.append(("file", node.channels["prompt"]))
        return {"receipt": f"T-{node.effect_key[:6]}"}

    graph = (
        ailu.create_graph("Triage")
        .channel("ticket", "string", default="")
        .channel("prompt", "string")
        .channel("receipt", "string")
        .node("prepare", prepare)
        .component("build", "promptBuilder", {"template": "Ticket: {{ticket}}", "into": "prompt"})
        .agent_node(
            "triage",
            system="Look the order up.",
            tools={"lookup": lambda tool_input: lookups.append(tool_input) or {"found": True}},
        )
        .human_gate("review")
        .node("file", file)
        .edge("prepare", "build")
        .edge("build", "triage")
        .conditional_edge(
            "triage", "review", "has_prompt", lambda channels: bool(channels.get("prompt"))
        )
        .edge("review", "file")
        .compile()
    )
    paused = graph.run({"ticket": "broken export"})
    assert paused["status"] == "suspended", paused
    assert paused["state"]["currentNodeId"] == "review"
    assert len(lookups) == 1
    assert graph.explain(paused["state"])["suspended"]["node"] == "review"
    done = graph.resume(paused["state"])
    assert done["status"] == "completed", done
    assert done["state"]["channels"]["receipt"].startswith("T-")
    assert calls == [("prepare", "broken export"), ("file", "Ticket: BROKEN EXPORT")]


def test_a_built_graph_with_a_subgraph_runs_the_child_step():
    seen = []

    def child_step(node):
        seen.append(node.channels.get("in"))
        return {"out": "done"}

    child = (
        ailu.create_graph("Child")
        .channel("in", "string", default="")
        .channel("out", "string")
        .node("child_step", child_step)
    )
    parent = (
        ailu.create_graph("Parent")
        .channel("ticket", "string", default="")
        .channel("result", "string")
        .subgraph("sub", child, input_mapping={"in": "ticket"}, output_mapping={"result": "out"})
        .compile()
    )
    outcome = parent.run({"ticket": "t-1"})
    assert outcome["status"] == "completed", outcome
    assert seen == ["t-1"]
    assert outcome["state"]["channels"]["result"] == "done"


def test_the_builder_refuses_a_graph_the_engine_finds_invalid():
    try:
        ailu.create_graph("Broken").node("a", _step).edge("a", "ghost").compile()
        raise AssertionError("expected a compile error")
    except ailu.GraphCompileError as error:
        assert error.errors[0]["code"] == "INVALID_EDGE_REFERENCE", error.errors
    for build in [
        lambda: ailu.create_graph("Dup").node("a", _step).human_gate("a"),
        lambda: (
            ailu.create_graph("Tools")
            .agent_node("x", system="s", tools={"t": lambda i: 1})
            .agent_node("y", system="s", tools={"t": lambda i: 2})
        ),
    ]:
        try:
            build()
            raise AssertionError("expected a ValueError")
        except ValueError:
            pass


def test_a_built_graph_replays_without_calling_its_steps():
    calls = []

    def send(node):
        calls.append(node.effect_key)
        return {"receipt": "r-1"}

    graph = ailu.create_graph("Send").channel("receipt", "string").node("send", send).compile()
    saved = os.environ.get("AILU_LLM_RECORD")
    os.environ["AILU_LLM_RECORD"] = "1"
    try:
        recorded = graph.run(run_id="run-built")
    finally:
        if saved is None:
            del os.environ["AILU_LLM_RECORD"]
        else:
            os.environ["AILU_LLM_RECORD"] = saved
    replayed = graph.replay(recorded["entryState"], "audit-1", recorded["replayJournal"])
    assert replayed["state"]["channels"]["receipt"] == "r-1"
    assert len(calls) == 1


# ---------------------------------------------------------------------------
# Embeddings and the vector store (ADR 0045 M4) — the TypeScript helpers' counterparts.
# ---------------------------------------------------------------------------

_EMBEDDINGS_GOLDEN = os.path.join(
    os.path.dirname(_CATALOG_GOLDEN), "embeddings_vectors_golden.json"
)


def test_embeddings_and_vector_search_match_every_golden_case():
    # Recorded from the TypeScript createEmbeddings / createVectorStore / cosineSimilarity.
    with open(_EMBEDDINGS_GOLDEN, encoding="utf-8") as golden_file:
        golden = json.load(golden_file)
    for case in golden["bodies"]:
        sent = []

        def transport(body, sent=sent, count=len(case["texts"])):
            sent.append(body)
            return {"data": [{"embedding": [0]}] * count}

        options = {
            {"baseUrl": "base_url", "apiKey": "api_key"}.get(key, key): value
            for key, value in case["options"].items()
        }
        ailu.create_embeddings(transport=transport, **options).embed(case["texts"])
        assert (sent[0] if sent else None) == case["expected"], case["name"]
    for case in golden["responses"]:
        embeddings = ailu.create_embeddings(transport=lambda body, r=case["response"]: r)
        try:
            got = {"vectors": embeddings.embed(["t"])}
        except ailu.RunError as error:
            got = {"error": f"createEmbeddings: {error}"}
        assert got == case["expected"], case["name"]
    for case in golden["queries"]:
        store = ailu.create_vector_store()
        store.upsert(case["items"])
        got = {"size": store.size(), "matches": store.query(case["embedding"], case["k"])}
        assert got == case["expected"], case["name"]
    for case in golden["cosines"]:
        assert ailu.cosine_similarity(case["a"], case["b"]) == case["expected"], case["name"]


def test_a_vector_store_saves_to_and_loads_from_its_file():
    import tempfile

    with tempfile.TemporaryDirectory() as directory:
        path = os.path.join(directory, "nested", "store.json")
        store = ailu.create_vector_store(path)
        store.upsert(
            [
                {"id": "a", "content": "alpha", "embedding": [1, 0]},
                {"id": "b", "content": "beta", "embedding": [0, 1], "metadata": {"page": 2}},
            ]
        )
        store.upsert([{"id": "a", "content": "alpha 2", "embedding": [0.5, 0.5]}])
        with open(path, encoding="utf-8") as saved:
            assert [item["id"] for item in json.load(saved)] == ["a", "b"]
        reloaded = ailu.create_vector_store(path)
        assert reloaded.size() == 2
        assert reloaded.query([0, 1], 1) == [
            {"id": "b", "content": "beta", "score": 1.0, "metadata": {"page": 2}}
        ]
        with open(path, "w", encoding="utf-8") as broken:
            broken.write('[{"id": "x"}, {"id": "y", "content": "y", "embedding": [true]}, "z"]')
        assert ailu.create_vector_store(path).size() == 0


def test_embeddings_without_a_key_name_the_variable_and_no_texts_make_no_call():
    key = os.environ.pop("MISTRAL_API_KEY", None)
    try:
        assert ailu.create_embeddings().embed([]) == []
        ailu.create_embeddings().embed(["x"])
        raise AssertionError("expected a missing-key error")
    except ailu.RunError as error:
        assert "MISTRAL_API_KEY" in str(error), str(error)
    finally:
        if key is not None:
            os.environ["MISTRAL_API_KEY"] = key


def _all_tests():
    return [value for name, value in sorted(globals().items()) if name.startswith("test_")]


if __name__ == "__main__":
    failures = 0
    for test in _all_tests():
        try:
            test()
            print(f"PASS {test.__name__}")
        except Exception as error:  # noqa: BLE001 - report-and-continue test runner
            failures += 1
            print(f"FAIL {test.__name__}: {error!r}")
    total = len(_all_tests())
    print(f"\n{total - failures}/{total} passed")
    sys.exit(1 if failures else 0)
