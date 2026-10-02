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
