---
title: "Python and other languages"
description: "What the Python SDK and the C ABI bindings can do today, and what still needs the TypeScript SDK."
---

# Python and other languages

The engine is written once, in Rust. The TypeScript SDK exposes all of it. Other languages
expose part of it today.

## Python

```bash
pip install adriane-ai
```

```python
import ailu

# Validate and compile graphs: the same checks as the TypeScript SDK.
graph = ailu.compile_graph_yaml(open("triage.graph.yaml").read())
assert ailu.validate_graph(graph) == []

# Resolve a model tier against the keys you have set.
print(ailu.available_providers())          # e.g. ["anthropic"]
print(ailu.resolve_model("fast"))          # {"provider": ..., "model": ..., "recommended": ...}

# Run one component, or one prebuilt agent.
ailu.run_component("promptBuilder", {"template": "Hello {{name}}!", "into": "prompt"}, {"name": "Ada"})
outcome = ailu.prebuilt.summarizer("A long text to summarize...")
print(outcome["status"], outcome["channels"])
```

| Function | Purpose |
| --- | --- |
| `validate_graph(definition)` | The list of problems in a graph definition (empty when valid). |
| `compile_graph_yaml(text)` | A YAML graph compiled to a definition (a dict). |
| `available_providers()`, `resolve_model(tier, ...)` | Which providers have keys, and which model a tier maps to. |
| `list_components()`, `run_component(kind, params, channels)` | The component catalog, and one component run. |
| `list_prebuilt()`, `run_prebuilt(name, input)`, `prebuilt.<name>(input)` | The prebuilt agents, and one agent run. |
| `GraphRunner(spec, nodes=, tools=, conditions=, on_event=)` | Runs a graph, with your functions as steps and tools: `run`, `resume`, `approve_and_resume`, `signal`, `replay`. |
| `run_catalog_graph(graph, ...)`, `resume_catalog_graph(...)`, `replay_catalog_graph(...)` | Runs a saved graph whose nodes carry their agent and component settings, like the TypeScript `runCatalogGraph`. |
| `explain_run(state, events=None)`, `verify_replay_decisions(attested, replayed)` | Where a run stands and what unblocks it; whether a replay reproduced the decisions a run was attested for. |

Errors are raised as `ailu.GraphValidationError`, `ailu.GraphCompileError` or `ailu.RunError`
(`ailu.HostNodeBindingError` and `ailu.ApprovalNotGrantedError` are `RunError`s).
Prebuilt agents read API keys like the TypeScript SDK does, and `AILU_LLM_MOCK=1` runs them
offline.

### Run a graph

`GraphRunner` runs a graph on the engine the TypeScript SDK uses. Give it the graph (a dict), the
agents it runs, and your functions: a plain action node can be a step of yours. Here a person
approves, then `send` acts:

```python
import ailu

graph = {
    "id": "send-after-approval", "version": "1", "name": "send-after-approval",
    "channels": {
        "message": {"type": "string", "reducer": "replace"},
        "receipt": {"type": "string", "reducer": "replace"},
    },
    "nodes": [
        {"id": "approve", "type": "human-gate", "label": "approve"},
        {"id": "send", "type": "action", "label": "send"},
    ],
    "edges": [{"id": "e1", "from": "approve", "to": "send", "type": "default"}],
    "entryNodeId": "approve",
}

def post_message(text: str) -> str:  # stands in for your messaging API
    return f"msg-{len(text)}"

receipts = {}

def send(node: ailu.HostNodeInput) -> dict:
    # The same effect key when this step runs again from the same checkpoint: send at most once.
    if node.effect_key not in receipts:
        receipts[node.effect_key] = post_message(node.channels["message"])
    return {"receipt": receipts[node.effect_key]}

runner = ailu.GraphRunner({"graph": graph}, nodes={"send": send})
paused = runner.run({"message": "Invoice 42 is paid."})   # status "suspended": nothing sent
done = runner.resume(paused["state"])                      # once a person approved
print(done["state"]["channels"]["receipt"])  # "msg-19"
```

Agents go in the spec's `agents`, keyed by node id (`{"provider": "anthropic", "system": ...,
"toolNames": [...], "outputChannel": ...}`); `tools={"lookup": fn}` backs the tools they call, and
`conditions={"approved": fn}` the named conditions of conditional edges. Functions are called
synchronously, on the calling thread.

Each call returns `{"state", "status", "pendingApprovals"}`. With `AILU_LLM_RECORD=1`, it also
returns `replayJournal` and, on a start, `entryState`: store both, and
`runner.replay(entry_state, "audit-1", replay_journal)` re-derives the run from them. A replay
never calls your steps or tools; it uses what they returned, and raises `ailu.RunError` if the run
reaches something its recording has no result for.

### Run a saved graph

A graph saved as data (in the Studio, or the `definition` of a TypeScript graph) carries its
agents, components and fan-outs as settings on its nodes: `metadata.agent`,
`metadata.component`, `metadata.mapAgents`. `run_catalog_graph` runs it. The engine reads those
settings itself, as it does for the TypeScript `runCatalogGraph`, so the graph runs the same from
both languages.

```python
import ailu

graph = {
    "id": "triage", "version": "1", "name": "triage",
    "channels": {
        "ticket": {"type": "string", "reducer": "replace"},
        "prompt": {"type": "string", "reducer": "replace"},
        "triaged": {"type": "agentResult", "reducer": "replace"},
        "receipt": {"type": "string", "reducer": "replace"},
    },
    "nodes": [
        {"id": "prompt", "type": "action", "label": "prompt", "metadata": {"component": {
            "kind": "promptBuilder",
            "params": {"template": "Triage this ticket: {{ticket}}", "into": "prompt"}}}},
        {"id": "triage", "type": "agent", "label": "triage", "metadata": {"agent": {
            "system": "Say how urgent the ticket is.", "outputChannel": "triaged"}}},
        {"id": "file", "type": "action", "label": "file"},
    ],
    "edges": [
        {"id": "e1", "from": "prompt", "to": "triage", "type": "default"},
        {"id": "e2", "from": "triage", "to": "file", "type": "default"},
    ],
    "entryNodeId": "prompt",
}

def file_ticket(node: ailu.HostNodeInput) -> dict:
    # Your code: record the triage in your tracker, once per effect key.
    return {"receipt": f"T-{node.effect_key[:8]}"}

outcome = ailu.run_catalog_graph(
    graph,
    initial_data={"ticket": "The export button does nothing."},
    nodes={"file": file_ticket},
)
print(outcome["status"])  # "completed"
```

The options are the TypeScript ones: `initial_data`, `run_id`, `nodes`, `tools`, `subgraphs`,
`provider_keys`, `fs_policy`, `skills`, `on_event`, `is_cancelled` and `approval_engine`
([below](#approvals)). A `nodes` id that names no
plain step raises `ailu.HostNodeBindingError`, and a malformed `mapAgents` setting a
`RuntimeWarning`. Conditional edges have no functions on this path, so they are not taken.

`resume_catalog_graph(graph, state, ...)` continues a suspended run: pass the same `nodes`,
`tools` and `subgraphs` (they are code, not state), and `approved_tools` to unlock tools a person
approved. `replay_catalog_graph(graph, entry_state, "audit-1", replay_journal)` re-derives a run
recorded with `AILU_LLM_RECORD=1`.

### Approvals

Give the runner an approval engine and it files a request for each gated tool an agent asks for
and for each human gate the run stops at. The resume then refuses to run until a person, other
than the one who asked, approved what the run waits on:

```python
approvals = ailu.InMemoryApprovalEngine()   # in production: your database (below)

paused = ailu.run_catalog_graph(graph, nodes={"file": file_ticket}, approval_engine=approvals)
for request in approvals.get_pending(paused["state"]["runId"]):
    approvals.approve(request["id"], "alice@example.com")   # from your authenticated session

done = ailu.resume_catalog_graph(
    graph, paused["state"], nodes={"file": file_ticket}, approval_engine=approvals
)
```

`resume_catalog_graph` raises `ailu.ApprovalNotGrantedError` (its `problems` say why) when a
request is still pending, a human gate was rejected, a request was approved by the agent or gate
that asked for it, a tool in `approved_tools` has no request approved by the person the grant
names, or the run was started without the engine. The engine makes these decisions, as for the
TypeScript `approvalEngine`; yours only stores the requests. In production, give it two methods
over your database: `request(*, run_id, node_id, requested_by, subject)`, which stores a pending
request and returns it with its `"id"`, and `get_by_id(request_id)`, which returns it with its
`"status"`, `"subject"`, `"requested_by"` and `"resolved_by"`.

### Explain a run, check a replay

`ailu.explain_run(state, events)` says where a run stands: its status, why and where it is
suspended and what unblocks it, what failed, its channel names (never their values) and its last
events. `ailu.verify_replay_decisions(attested, replayed)` compares, in order, the
`{"status", "subject"}` decisions a run was attested for with those its replay made, and lists
each mismatch. Both are the engine's, as in TypeScript (`explainRun`, `verifyReplayDecisions`);
the next steps the explanation names are the TypeScript calls (`app.resume(runId)`), which are
`resume_catalog_graph` or a `GraphRunner` method in Python.

### Wait for a date or a signal

A step can suspend the run until a date or an external event. The engine keeps no clock: your
scheduler resumes the run when `wakeAt` comes, or you deliver the signal.

```python
def remind(node: ailu.HostNodeInput) -> dict:
    return ailu.sleep_until("2026-10-03T08:00:00Z", {"status": "waiting"})

def wait_payment(node: ailu.HostNodeInput) -> dict:
    return ailu.wait_for_signal("paid", wake_at="2026-10-10T00:00:00Z")  # or time out

paused = runner.run()
ailu.read_suspend_meta(paused["state"])   # {"reason": "signal", "awaitingSignal": "paid", "wakeAt": ...}
done = runner.signal(paused["state"], "paid", {"amount": 42})
# a later step reads it: ailu.read_signal(node, "paid") == {"amount": 42}
```

`runner.resume(state)` (or `resume_catalog_graph`) continues a timer past its step.

### Stream tokens

`run_catalog_graph(..., stream_tokens=True)` (or `runner.run(stream_tokens=True)`) sends each
agent's reply to `on_event` as it is generated, as `{"type": "token_delta", "nodeId",
"messageId", "delta"}` events. The run's result is the same either way.

:::note Not yet in Python
A graph builder and calling a model directly are TypeScript only for now. They come to Python
in later releases.
:::

## Other languages

The `sdks/` folder of the repository has bindings for Go, Java/Kotlin (JVM), C#, C++, Swift,
Objective-C, Zig, Ruby, PHP, Lua, PowerShell and Elixir, over the engine's C ABI
(`include/ailu.h`). They are **experimental**: the surface may change, and they are not published
to package registries. Build them from source; each folder has its own README.

The C ABI runs graphs with your callbacks for steps, tools, conditions and events. To cancel a
run, use its `_v2` entry points (`ailu_engine_run_json_v2`, …): their `AiluCallbacksV2` adds
`is_cancelled`, asked at every node boundary. The C++ wrapper has them; the other wrappers do not
yet.
