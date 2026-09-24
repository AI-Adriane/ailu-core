# Ailu — Python SDK

A thin, Pythonic SDK over the Ailu **Rust engine**, exposed through a
[pyo3](https://pyo3.rs) native extension module.

> **Install `ailu`, import `ailu`.** The PyPI distribution and the import package share
> the same name.

```bash
pip install ailu
```

```python
import ailu

ailu.engine_version()   # -> the bound Rust engine version, e.g. "0.1.0"
```

## One engine, two SDKs — what to install where

The graph model, validator, and DSL compiler live **once** in Rust (under
`crates/`). Each language SDK is a thin shim over that single engine, not a
re-implementation — so a graph that validates one way in TypeScript validates
exactly the same way in Python. There is no second source of truth to drift.

| | TypeScript | Python (this package) |
| --- | --- | --- |
| Install | `npm i @ailu-ai/graph-sdk` | `pip install ailu` |
| Import | `import { createGraph } from "@ailu-ai/graph-sdk"` | `import ailu` |
| Rust engine | **optional** — `@ailu-ai/napi` activates it; falls back to the in-bundle TS engine when absent | **built in** — the wheel ships the compiled pyo3 extension |
| Bridge | [napi-rs](https://napi.rs) (`crates/bindings`) | [pyo3](https://pyo3.rs) (`crates/py-bindings`) |
| Surface | full builder + custom handlers + streaming | JSON-in / JSON-out: validate, compile, model policy, catalogs, run paths |

Both bindings expose the identical JSON-in / JSON-out core (graph validation, DSL
compilation, the model policy, the component/prebuilt catalogs, and the
fully-Rust run paths). The TypeScript SDK adds a builder and custom node handlers
on top; the Python SDK is the thin JSON surface.

## API

### Graph model & DSL

```python
import ailu

ailu.engine_version()            # -> the bound Rust engine version string

ailu.validate_graph({            # -> list[dict] of validation errors ([] if sound)
    "id": "g", "version": "0.0.0", "name": "g", "channels": {},
    "nodes": [{"id": "a", "type": "action", "label": "a"}],
    "edges": [{"id": "e1", "from": "a", "to": "ghost", "type": "default"}],
    "entryNodeId": "a",
})
# [{'code': 'INVALID_EDGE_REFERENCE', 'message': "Edge 'e1' references unknown node 'ghost'.", 'path': ['e1']}]

ailu.compile_graph_yaml("""    # -> dict (a compiled GraphDefinition)
id: g
version: 0.0.0
name: g
entryNodeId: a
nodes:
  - id: a
    type: action
    label: A
edges: []
channels: {}
""")
```

`validate_graph` returns the full list of structural errors (it does not raise on
an invalid-but-parseable graph). `compile_graph_yaml` raises `ValueError`
(`ailu.GraphCompileError`) when the DSL fails to parse, compile, or validate.

### Model policy

```python
ailu.available_providers()       # -> list[str], from process env credentials
# e.g. ["mistral"] when MISTRAL_API_KEY is set; [] when none are.

ailu.resolve_model("fast", available=["mistral"])
# -> {'provider': 'mistral', 'model': 'mistral-small-latest', 'recommended': True}

# Tiers: "frontier" | "balanced" | "fast" | "creative".
# Omit `available` to derive it from the env. A provider/model override wins
# over the policy choice and flags `recommended = False`:
ailu.resolve_model("frontier", available=["anthropic"], provider="mistral", model="mistral-tiny")
# -> {'provider': 'mistral', 'model': 'mistral-tiny', 'recommended': False}
```

### Catalogs

```python
ailu.list_components()   # -> list[str] of the component kinds, e.g. "promptBuilder"
ailu.list_prebuilt()     # -> list[dict] of the 16 prebuilt micro-agents
# each: {'name', 'description', 'tier', 'systemPrompt', 'toolNames',
#        'suspendForApproval', 'outputChannel'}  (camelCase, from the Rust engine)
```

### Run paths (fully on Rust)

Both runs execute end-to-end in Rust — no Python callbacks. When no provider
credentials are present in the env, `run_prebuilt` falls back to a deterministic
mock gateway, so a run still completes offline.

```python
ailu.run_component(              # -> dict, the component's channel-update map
    "promptBuilder",
    {"template": "Hello {{name}}!", "into": "prompt"},
    {"name": "Ada"},
)
# {'prompt': 'Hello Ada!'}

ailu.run_prebuilt("summarizer", "please summarise this long text")
# -> {'status': 'completed',
#     'channels': {'input': ..., 'summary': {...}},
#     'resolvedModel': {'provider': 'mock', 'model': 'mock-model'}}

# Ergonomic accessor: each attribute is bound to that agent name.
ailu.prebuilt.summarizer("please summarise this long text")   # same as run_prebuilt("summarizer", ...)
ailu.prebuilt.classifier("is this spam?", provider="mistral") # override forwarded through
```

`run_component` and `run_prebuilt` raise `ValueError` (`ailu.RunError`) on an
unknown kind/agent, invalid input, or an engine/runtime failure.

## Install

A single `cp39-abi3` wheel covers CPython 3.9+ (the extension targets the stable
ABI), so nothing compiles on the user's machine:

```bash
pip install ailu
```

```python
import ailu
print(ailu.engine_version())
```

### From source (dev)

The package is built with [maturin](https://www.maturin.rs) over the Rust
workspace crate `crates/py-bindings`, driven by `python/pyproject.toml`:

```bash
. "$HOME/.cargo/env"
python3 -m venv .venv && source .venv/bin/activate
pip install maturin

cd python
maturin develop            # build the extension + install into the active venv
# …or build a distributable wheel:
maturin build --release    # -> target/wheels/ailu-<version>-cp39-abi3-*.whl

python -c "import ailu; print(ailu.engine_version())"
```

`maturin` compiles the pyo3 cdylib and places it as the `ailu.ailu`
submodule — the leaf import name `ailu` resolves the `PyInit_ailu` symbol
emitted by `#[pymodule] fn ailu` in `crates/py-bindings/src/lib.rs`.

## Tests

```bash
cd python
maturin develop                 # build + install the extension into the venv
python -m pytest tests -q       # if pytest is installed
python tests/test_ailu.py    # plain-assert fallback when pytest is absent
```
