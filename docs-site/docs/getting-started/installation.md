---
sidebar_position: 1
title: Installation
description: Install the Ailu SDK in TypeScript, Python, or a C-ABI language.
---

# Installation

Ailu ships SDKs over **one Rust engine**. Pick your language: the package you install, the
name you import, and the native library you load differ by ecosystem convention.

:::tip Naming at a glance
| | Install | Import | Command |
| --- | --- | --- | --- |
| **TypeScript** | `npm i @ailu/graph-sdk` | `import { createGraph } from "@ailu/graph-sdk"` | — |
| **Python** | `pip install ailu` | `import ailu` | — |
| **C-ABI SDKs** | build `ailu-c-api` | language-specific wrapper in `sdks/` | `AILU_C_API_LIB` |
| **CLI** | `npm i -g @ailu/cli` | — | `ailu` |

The npm scope is `@ailu`. On PyPI the distribution is `ailu` (hyphen), but the
**import package is `ailu`** (underscore) — Python module names can't contain a
hyphen, so this is the standard pip↔import split, the same as `pip install scikit-learn` /
`import sklearn`.
:::

:::caution One engine, many SDKs, not byte-identical surfaces
The Rust engine is shared, so validation, DSL compilation, model policy, catalogs, native
component runs, and prebuilt-agent runs come from the same code. The callback-heavy TypeScript
features, such as custom node handlers, streaming `stream()`, and in-process `conditionalEdge`
predicates, are available through N-API and the C callback ABI. Python remains JSON-in/JSON-out
until its PyO3 layer gets the same callback runtime. Skim **[One engine, many languages](/docs/sdk-parity/one-engine-two-languages)**
first so the parity boundaries are explicit.
:::

## TypeScript

```bash
npm i @ailu/graph-sdk
# or: pnpm add @ailu/graph-sdk   /   yarn add @ailu/graph-sdk
```

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "hello" })
  .node("greet", async () => ({ greeting: "hello world" }))
  .compile();

const result = await app.run({});
console.log(result.status); // "completed"
```

`@ailu/graph-sdk` is a **self-contained bundle**, and it depends on the Rust engine
(`@ailu/napi`) — so `npm i @ailu/graph-sdk` pulls the engine for you. No extra step.

:::tip Fastest start — scaffold a governed app
```bash
npm create ailu@latest my-app   # a runnable governed graph + the dev inspector
cd my-app && npm install && npm start
```
:::

### The Rust engine is required

Ailu runs on the **Rust engine**. `@ailu/napi` is a regular **dependency** of the SDK,
installed automatically; you don't install it separately and you don't opt in to it. You can
confirm it's active:

```ts
import { rustEngineAvailable } from "@ailu/graph-sdk";

console.log(rustEngineAvailable()); // true — the Rust engine is running
```

:::note No TypeScript execution fallback
Execution is **Rust-only**: if the native addon genuinely can't run a graph, the SDK throws
`RustEngineRequiredError` rather than silently degrading (ADR 0016). Prebuilt addons cover
macOS, Linux **glibc**, and Windows (x64/arm64); on an uncovered target (e.g. **musl/Alpine**)
either use a glibc base image (`node:20-slim`) or install the Rust toolchain so the addon builds
from source. The TypeScript engine packages remain only as an internal dev/test aid, never a
runtime you target.
:::

## Python

```bash
pip install ailu
```

```python
import ailu

print(ailu.engine_version())   # the bound Rust engine version
```

A single `cp39-abi3` wheel covers CPython 3.9+ — the extension targets the stable ABI, so
**nothing compiles on your machine** and the Rust engine is **always present** (there is no
fallback path in Python; the wheel *is* the engine).

The Python SDK is a thin JSON-in / JSON-out surface over the engine: graph validation, DSL
compilation, the model policy, the component and prebuilt catalogs, and the fully-Rust run
paths. See the [Python SDK](/docs/sdk-parity/python-sdk) page for the full surface.

## CLI

```bash
npm i -g @ailu/cli
ailu --help
```

The npm package is `@ailu/cli`; the installed command is `ailu`. It bundles the
engine, so it runs the moment it's installed. See [the CLI reference](/docs/cli/commands).

## From source (contributors)

The engine and both SDKs build from the monorepo. You need Node 18+ with pnpm, and a Rust
toolchain for the native engine.

```bash
pnpm install
pnpm build

# Native engine (the SDK's required runtime; also builds the Python wheel locally):
pnpm napi:build   # builds the Node addon  → engine/crates/bindings/ailu_napi.node
pnpm py:build     # builds the Python ext   → python/ailu/ailu.abi3.so
```

## Next

[Your first run →](/docs/getting-started/your-first-run)
