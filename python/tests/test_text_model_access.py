"""Admission boundary through real Python native bindings; all provider calls are mocked."""
import hashlib
import json
import os
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from unittest.mock import patch

import ailu


def policy(run_id="policy-run", allowed=None):
    models = [{"provider": "mock", "modelId": "mock"}] if allowed is None else allowed
    # Host owns canonicalization. UTF-16 ordering is exercised by the native TS fixture;
    # this ASCII fixture deliberately does not introduce a second production hasher.
    definition = {"schemaVersion": 1, "catalogRevision": "test", "enabledModels": models}
    return {
        "schemaVersion": 1, "tenantId": "tenant-a", "runId": run_id,
        "policyId": "organization-text-models", "policyVersion": 1,
        "catalogRevision": "test",
        "policyHash": hashlib.sha256(json.dumps(definition, separators=(",", ":")).encode()).hexdigest(),
        "allowedModels": models,
    }


COUNTED = {"provider": "ollama", "modelId": "counted"}
DENIED = "text_model_access_denied"


class CountingProvider(BaseHTTPRequestHandler):
    """The provider of the refused runs: counts every request the engine sends it."""
    calls = 0

    def do_POST(self):
        CountingProvider.calls += 1
        self.rfile.read(int(self.headers.get("content-length", 0)))
        body = json.dumps({"choices": [{"index": 0, "finish_reason": "stop",
                                        "message": {"role": "assistant", "content": "FINAL: done"}}],
                           "usage": {"prompt_tokens": 1, "completion_tokens": 1}}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def graph(gated=False, provider="mock", model="mock"):
    agent = {"id": "agent", "type": "agent", "label": "agent", "metadata": {
        "agent": {"provider": provider, "model": model, "maxIterations": 1, "outputChannel": "answer"}
    }}
    return {"id": "policy", "version": "1", "name": "policy",
            "channels": {"answer": {"type": "string", "reducer": "replace", "default": ""}},
            "nodes": ([{"id": "gate", "type": "human-gate", "label": "gate"}] if gated else []) + [agent],
            "edges": [{"id": "e", "from": "gate", "to": "agent", "type": "default"}] if gated else [],
            "entryNodeId": "gate" if gated else "agent"}


class TextModelAccessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.provider = HTTPServer(("127.0.0.1", 0), CountingProvider)
        threading.Thread(target=cls.provider.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.provider.shutdown()
        cls.provider.server_close()

    def assert_refused(self, outcome, events, node_id="agent"):
        """Failed on exactly the denial code, no provider call, nothing of it in the state."""
        self.assertEqual(outcome["status"], "failed")
        self.assertEqual(outcome["state"]["status"], "failed")
        failures = [e for e in events if e["type"].endswith("_failed")]
        self.assertEqual([(e["type"], e.get("nodeId"), e["error"]) for e in failures],
                         [("node_failed", node_id, DENIED), ("run_failed", None, DENIED)])
        self.assertFalse(any(e["type"] in ("node_completed", "token_delta") for e in events))
        self.assertEqual(CountingProvider.calls, 0)
        self.assertNotIn(DENIED, json.dumps(outcome))
        self.assertNotIn("error", json.dumps(outcome["state"]["channels"]))

    def setUp(self):
        CountingProvider.calls = 0
        self.env = patch.dict(os.environ, {
            "AILU_LLM_MOCK": "1", "AILU_USE_OLLAMA": "1",
            "AILU_OLLAMA_BASE_URL": f"http://127.0.0.1:{self.provider.server_port}/v1"})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.assertEqual(ailu.text_model_access_version(), 1, "test requires the current native addon")

    def test_capability_requires_the_actual_integer_native_version(self):
        for unsupported in (True, 1.0, "1", 0, 2, None):
            with patch.object(ailu._native, "text_model_access_version", return_value=unsupported):
                self.assertIsNone(ailu.text_model_access_version())
        with patch.object(ailu._native, "text_model_access_version", side_effect=RuntimeError("unavailable")):
            self.assertIsNone(ailu.text_model_access_version())

    def test_standalone_allow_deny_and_cross_tenant(self):
        opts = {"provider": "mock", "model": "mock", "tenant_id": "tenant-a", "run_id": "policy-run"}
        result = ailu.llm_complete("hello", **opts, text_model_access=policy())
        self.assertEqual(result["provider"], "mock")
        with self.assertRaisesRegex(ailu.RunError, "text_model_access_denied"):
            ailu.llm_complete("hello", **opts, text_model_access=policy(allowed=[]))
        with self.assertRaisesRegex(ailu.RunError, "text_model_access_invalid"):
            ailu.llm_complete("hello", **{**opts, "tenant_id": "other"}, text_model_access=policy())

    def test_the_counting_provider_is_called_when_its_model_is_allowed(self):
        outcome = ailu.run_catalog_graph(graph(provider="ollama", model="counted"),
                                         run_id="policy-run", tenant_id="tenant-a",
                                         text_model_access=policy(allowed=[COUNTED]))
        self.assertEqual(outcome["status"], "completed")
        self.assertEqual(CountingProvider.calls, 1)

    def test_catalog_complete_stream_and_legacy(self):
        for streaming in (False, True):
            events = []
            snapshot = policy(allowed=[])
            outcome = ailu.run_catalog_graph(graph(provider="ollama", model="counted"),
                                             run_id="policy-run", tenant_id="tenant-a",
                                             text_model_access=snapshot, stream_tokens=streaming,
                                             on_event=events.append)
            self.assertEqual(outcome["state"]["textModelAccess"], snapshot)
            self.assertNotIn("textModelAccess", outcome["state"]["channels"])
            self.assert_refused(outcome, events)
        legacy = ailu.run_catalog_graph(graph(), run_id="legacy")
        self.assertNotIn("textModelAccess", legacy["state"])

    def test_resume_same_binding_missing_and_replaced(self):
        snapshot = policy()
        outcome = ailu.run_catalog_graph(graph(True), run_id="policy-run", tenant_id="tenant-a",
                                         text_model_access=snapshot)
        self.assertEqual(outcome["status"], "suspended")
        for kwargs in ({}, {"tenant_id": "tenant-a", "text_model_access": policy(allowed=[])}):
            with self.assertRaisesRegex(ailu.RunError, "text_model_access_snapshot_mismatch"):
                ailu.resume_catalog_graph(graph(True), outcome["state"], **kwargs)
        resumed = ailu.resume_catalog_graph(graph(True), outcome["state"], tenant_id="tenant-a",
                                            text_model_access=snapshot)
        self.assertEqual(resumed["status"], "completed")

    def test_older_addon_rejects_all_entries_and_explicit_null_before_io(self):
        base = {"graph": graph(), "textModelAccess": None}
        state = {"runId": "policy-run", "channels": {}, "textModelAccess": None}
        with patch.object(ailu._native, "text_model_access_version", return_value=0), \
             patch.object(ailu._native, "engine_run") as start, \
             patch.object(ailu._native, "engine_resume") as resume, \
             patch.object(ailu._native, "engine_approve_and_resume") as approve, \
             patch.object(ailu._native, "engine_signal") as signal, \
             patch.object(ailu._native, "engine_replay") as replay, \
             patch.object(ailu._native, "llm_complete") as complete, \
             patch.object(ailu._native, "engine_spec_from_catalog") as catalog:
            runner = ailu.GraphRunner({"graph": graph()})
            calls = [lambda: ailu.GraphRunner(base).run(), lambda: runner.resume(state),
                     lambda: runner.approve_and_resume(state, []), lambda: runner.signal(state, "x"),
                     lambda: runner.replay(state, "cp", "{}"),
                     lambda: ailu.run_catalog_graph(graph(), text_model_access=policy()),
                     lambda: ailu.llm_complete("x", provider="mock", text_model_access=policy())]
            for call in calls:
                with self.assertRaises(ailu.TextModelAccessUnsupportedError):
                    call()
            for native_call in (start, resume, approve, signal, replay, complete, catalog):
                native_call.assert_not_called()

    def test_explicit_null_new_addon_is_invalid(self):
        with self.assertRaisesRegex(ailu.RunError, "text_model_access_invalid"):
            ailu.GraphRunner({"graph": graph(), "textModelAccess": None}).run()

    def test_builder_carries_admission_and_retains_legacy_defaults(self):
        compiled = (ailu.create_graph("Governed")
                    .agent_node("agent", system="Help.", provider="ollama", model="counted")
                    .compile(tenant_id="tenant-a", text_model_access=policy(allowed=[])))
        events = []
        outcome = compiled.run(run_id="policy-run", on_event=events.append)
        self.assert_refused(outcome, events)
        self.assertEqual(outcome["state"]["textModelAccess"], policy(allowed=[]))

    def test_raw_catalog_explicit_null_is_not_silently_removed(self):
        with self.assertRaisesRegex(ValueError, "text_model_access_invalid"):
            ailu._native.engine_spec_from_catalog(json.dumps({"graph": graph(), "textModelAccess": None}))

    def test_legacy_prebuilt_raw_entry_refuses_governed_options(self):
        with self.assertRaisesRegex(ValueError, "text_model_access_unsupported_entry"):
            ailu._native.run_prebuilt("summarizer", '"hello"', json.dumps({"textModelAccess": policy()}))


if __name__ == "__main__":
    unittest.main()
