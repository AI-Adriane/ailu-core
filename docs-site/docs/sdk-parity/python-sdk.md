---
sidebar_position: 3
title: Python SDK
description: Use Ailu from Python — validate, compile, run components and prebuilt agents.
---

# Python SDK

The Python SDK is a thin, Pythonic wrapper over the **same Rust engine** the TypeScript SDK
uses. The boundary is **JSON in / JSON out**; the module hides that, taking and returning native
Python `dict` / `list` values.

```bash
pip install ailu
```

```python
import ailu
```

:::tip Install vs import
The PyPI distribution and the import package share one name: `pip install ailu`, then
`import ailu`.
:::

## What's in the module

```python
import ailu

ailu.engine_version()        # version string of the bound Rust engine
ailu.validate_graph(dict)    # -> list of validation-error dicts (empty == valid)
ailu.compile_graph_yaml(str) # -> compiled GraphDefinition dict
ailu.available_providers()   # -> provider ids enabled by env credentials
ailu.resolve_model(tier, …)  # -> a {provider, model, recommended} choice
ailu.list_components()       # -> component-kind strings
ailu.list_prebuilt()         # -> prebuilt-agent definition dicts
ailu.run_component(kind, params, channels)  # run one component, on Rust
ailu.run_prebuilt(name, input, …)           # run a prebuilt micro-agent, on Rust
ailu.prebuilt                # ergonomic accessor: ailu.prebuilt.summarizer(text)
```

## Validate a graph

`validate_graph` takes a graph definition as a plain `dict` and returns a **list of error
dicts** — an empty list means the graph is structurally sound. (It raises
`GraphValidationError` only if the input can't even be encoded as JSON.)

```python
import ailu

definition = {
    "id": "greeter",
    "version": "1.0.0",
    "name": "Greeter",
    "entryNodeId": "n1",
    "channels": {"greeting": {"type": "string", "reducer": "replace"}},
    "nodes": [{"id": "n1", "type": "action", "label": "Start"}],
    "edges": [],
}

errors = ailu.validate_graph(definition)
print(errors)  # [] when valid; otherwise [{"code": ..., "message": ..., "path": ...}, ...]
```

## Compile DSL YAML

`compile_graph_yaml` compiles an Ailu graph DSL document into a validated `GraphDefinition`
dict. It raises `GraphCompileError` on a parse, DSL, or validation failure.

```python
import ailu

definition = ailu.compile_graph_yaml("""
id: graph-1
version: 1.0.0
name: Demo graph
entryNodeId: n1
channels:
  messages:
    type: messages
    reducer: append
nodes:
  - id: n1
    type: action
    label: Start
edges: []
""")
print(definition["id"])    # "graph-1"
```

## Run a pure component

`run_component(kind, params, channels)` runs a single component handler fully on Rust and
returns its **channel-update map** (the output patch).

```python
import ailu

out = ailu.run_component(
    "promptBuilder",
    {"template": "Hi {{name}}!", "into": "prompt"},
    {"name": "Ada"},
)
print(out)  # {"prompt": "Hi Ada!"}
```

Use `ailu.list_components()` to see every kind the engine knows.

## Run a prebuilt micro-agent

`run_prebuilt(name, input, provider=None, model=None)` runs a prebuilt agent on Rust. The
agent's model is resolved from its tier and the env-available providers; with no credentials it
falls back to a **deterministic mock**, so a run still completes offline.

```python
import ailu

result = ailu.run_prebuilt("summarizer", "A long passage of text to condense…")
print(result["status"])         # e.g. "completed"
print(result["resolvedModel"])  # {"provider": ..., "model": ...}

# Ergonomic accessor — shorthand for run_prebuilt:
ailu.prebuilt.summarizer("…long text…")
ailu.prebuilt.classifier("I love this!")
```

## Resolving a model tier

```python
import ailu

choice = ailu.resolve_model("balanced")
print(choice)  # {"provider": "...", "model": "...", "recommended": True}
```

`resolve_model(tier, available=None, *, provider=None, model=None)` maps a capability tier
(`"frontier" | "balanced" | "fast" | "creative"`) to a concrete `{provider, model,
recommended}`. With `available=None` the providers come from the environment. An explicit
`provider`/`model` override wins and is flagged `"recommended": False`.

## Errors

Three typed errors, all `ValueError` subclasses:

- `GraphValidationError` — the definition can't be encoded/parsed as JSON.
- `GraphCompileError` — DSL YAML fails to parse, compile, or validate.
- `RunError` — an unknown component kind / agent name, invalid params/input, or a runtime
  failure reported by the engine.

:::note Why no custom nodes yet
The binding is JSON-in / JSON-out with no Python callbacks crossing into Rust, so the Python
surface is validation, compilation, single-component runs, and prebuilt-agent runs. The C ABI now
has the callback runtime; Python still needs an equivalent PyO3 callback layer before custom
Python node handlers and streaming can match TypeScript.
:::

## Next

- [The Ailu DSL](/docs/dsl/graph-yaml-syntax) — author graphs as YAML from either language.
- [One engine, many languages](./one-engine-two-languages)
