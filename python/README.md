# Ailu — Python SDK

A thin, Pythonic SDK over the Ailu **Rust engine**, exposed through a
[pyo3](https://pyo3.rs) native extension module.

> **Install `ailu`, import `ailu`.** The PyPI distribution and the import package share
> the same name.

```bash
pip install adriane-ai
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
| Install | `npm i @ailu-ai/graph-sdk` | `pip install adriane-ai` |
| Import | `import { createGraph } from "@ailu-ai/graph-sdk"` | `import ailu` |
| Rust engine | **built in** — the native addon `@ailu-ai/napi` is a dependency; no TypeScript fallback | **built in** — the wheel ships the compiled pyo3 extension |
| Bridge | [napi-rs](https://napi.rs) (`crates/bindings`) | [pyo3](https://pyo3.rs) (`crates/py-bindings`) |
| Surface | full builder + custom handlers + streaming | validate, compile, model policy, catalogs, run a component or a prebuilt agent, `GraphRunner`: run, resume, approve, signal and replay a graph with your functions as steps, tools and conditions, and `run_catalog_graph` for saved graphs (no graph builder or streaming yet) |

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
ailu.list_prebuilt()     # -> list[dict] of the prebuilt micro-agents
# each: {'name', 'description', 'tier', 'systemPrompt', 'toolNames',
#        'suspendForApproval', 'outputChannel'}  (camelCase, from the Rust engine)
```

### Run paths (fully on Rust)

Both runs execute end-to-end in Rust — no Python callbacks. A prebuilt agent reads its
provider's API key from the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …); with no
key it raises an error naming the variable to set. To run offline on the engine's
deterministic mock gateway (tests, CI), set `AILU_LLM_MOCK=1`.

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

### Graph runner

`GraphRunner` drives the same engine runner as the TypeScript SDK, over an engine spec
(`{"graph": ..., "agents": ..., ...}`, camelCase keys). Your functions are the parts the engine
cannot run itself:

```python
def send(node: ailu.HostNodeInput) -> dict:
    # node.channels: the run's channels; node.effect_key: the same for a retry of this step
    # from the same checkpoint — perform an external effect at most once per key.
    return {"receipt": post_message(node.channels["message"])}

runner = ailu.GraphRunner(
    {"graph": graph},                       # a GraphDefinition dict
    nodes={"send": send},                   # plain action nodes whose step is yours
    tools={"lookup": lambda query: [...]},  # tools agents may call
    conditions={"approved": lambda channels: channels["ok"]},
    on_event=print,                         # every run lifecycle event
)
outcome = runner.run({"message": "hi"}, run_id="run-1", is_cancelled=lambda: False)
outcome = runner.resume(outcome["state"])                       # past a human gate
outcome = runner.approve_and_resume(state, [{"name": "refund", "requestedBy": "agent", "resolvedBy": "alice"}])
outcome = runner.signal(state, "paid", {"amount": 42})          # to a run waiting on a signal
replayed = runner.replay(entry_state, "audit-1", replay_journal)  # never calls your functions
```

Each call returns `{"state", "status", "pendingApprovals"}`, plus `replayJournal` and
`entryState` when `AILU_LLM_RECORD=1`. A replay serves steps and tools from the recording and
raises `ailu.RunError` when it diverges from it.

### Saved graphs

A graph saved with its agents, components and fan-outs as settings on its nodes
(`metadata.agent`, `metadata.component`, `metadata.mapAgents`) runs as it is: the engine reads
those settings, as for the TypeScript `runCatalogGraph`.

```python
outcome = ailu.run_catalog_graph(
    graph,                                  # the saved GraphDefinition dict
    initial_data={"ticket": "The export button does nothing."},
    nodes={"file": file_ticket},            # plain steps that are yours
    tools={"lookup": lookup},               # tools its agents call
    subgraphs=[child],                      # graphs its subgraph nodes name
)
outcome = ailu.resume_catalog_graph(graph, outcome["state"], nodes={"file": file_ticket})
replayed = ailu.replay_catalog_graph(graph, entry_state, "audit-1", replay_journal)
```

A `nodes` id that names no plain step raises `ailu.HostNodeBindingError`. The TypeScript SDK goes
further today (approvals filed with an approval engine, a graph builder, token streaming); Python
follows in later releases.

## Install

A single `cp39-abi3` wheel covers CPython 3.9+ (the extension targets the stable
ABI), so nothing compiles on the user's machine:

```bash
pip install adriane-ai
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
