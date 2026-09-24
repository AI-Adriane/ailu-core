---
title: "Python and other languages"
description: "What the Python SDK and the C ABI bindings can do today, and what still needs the TypeScript SDK."
---

# Python and other languages

The engine is written once, in Rust. The TypeScript SDK exposes all of it. Other languages
expose part of it today.

## Python

```bash
pip install ailu
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

Errors are raised as `ailu.GraphValidationError`, `ailu.GraphCompileError` or `ailu.RunError`.
Prebuilt agents read API keys like the TypeScript SDK does, and `AILU_LLM_MOCK=1` runs them
offline.

:::note Not yet in Python
Building a graph, running it, human gates, resume and streaming are TypeScript only for now. A
Python service can run the graphs through a small TypeScript worker.
:::

## Other languages

The `sdks/` folder of the repository has bindings for Go, Java/Kotlin (JVM), C#, C++, Swift,
Objective-C, Zig, Ruby, PHP, Lua, PowerShell and Elixir, over the engine's C ABI
(`include/ailu.h`). They are **experimental**: the surface may change, and they are not published
to package registries. Build them from source; each folder has its own README.
