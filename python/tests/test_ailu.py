"""Tests for the Ailu Python SDK.

Designed to run two ways:

  * ``pytest`` (collects the ``test_*`` functions), and
  * ``python -m tests.test_ailu`` / ``python tests/test_ailu.py`` when
    pytest is not installed — the ``__main__`` block runs every test with plain
    asserts and reports a pass/fail summary.

Both paths exercise the same Rust engine that backs the TypeScript SDK.
"""

from __future__ import annotations

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
