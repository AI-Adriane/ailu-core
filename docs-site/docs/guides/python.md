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

Errors are raised as `ailu.GraphValidationError`, `ailu.GraphCompileError` or `ailu.RunError`.
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

:::note Not yet in Python
A graph builder, running a saved graph whose nodes carry agent and component settings
(`runCatalogGraph`), token streaming, calling a model directly and the durable helpers
(`sleepUntil`, `waitForSignal`) are TypeScript only for now. They come to Python in later
releases.
:::

## Other languages

The `sdks/` folder of the repository has bindings for Go, Java/Kotlin (JVM), C#, C++, Swift,
Objective-C, Zig, Ruby, PHP, Lua, PowerShell and Elixir, over the engine's C ABI
(`include/ailu.h`). They are **experimental**: the surface may change, and they are not published
to package registries. Build them from source; each folder has its own README.
