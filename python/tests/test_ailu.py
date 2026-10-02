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
