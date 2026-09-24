# Ailu

[![CI](https://github.com/AI-Adriane/ailu-core/actions/workflows/unit.yml/badge.svg)](https://github.com/AI-Adriane/ailu-core/actions/workflows/unit.yml)
[![Rust](https://github.com/AI-Adriane/ailu-core/actions/workflows/rust.yml/badge.svg)](https://github.com/AI-Adriane/ailu-core/actions/workflows/rust.yml)
[![CodeQL](https://github.com/AI-Adriane/ailu-core/actions/workflows/codeql.yml/badge.svg)](https://github.com/AI-Adriane/ailu-core/actions/workflows/codeql.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

**The open framework for stateful, resumable agent graphs — with a governed studio on top.**

Ailu orchestrates agents, tools, human-approval gates, artifacts and
long-running workflows as a **graph**. The execution engine is written in **Rust**
(see [`crates/`](crates/)) and driven through thin **TypeScript and Python SDKs**.
Every run is deterministic by default, checkpointed after every step, and resumable
from where it stopped — including across process restarts and human approvals.

Ailu is split in two — an open framework and a commercial studio built on it:

| | Ailu |
| --- | --- |
| **The framework** (open source, self-hostable) | The Rust engine, the SDKs (TypeScript + Python) and the CLI. Build and run graphs anywhere — your laptop, your servers, your CI. |
| **Ailu Studio** (commercial, hosted) | The visual control plane: graph builder, fleet management, observability/tracing, evaluation, multi-tenant governance and approvals. |

The framework is, and stays, open. Studio is the paid product built on top of it.

---

## Quickstart (5 minutes)

> **Pre-release** — the SDK API may still change before 1.0. The Rust engine ships
> **prebuilt** (macOS · Linux · Windows, x64/arm64), so there is **no Rust toolchain to
> install** — `npm install` pulls the right binary and you run on Rust immediately.

```bash
npm install @ailu/graph-sdk      # or: pnpm add / yarn add
```

> **Platforms.** Prebuilt engines ship for macOS (x64/arm64), Linux **glibc** (x64/arm64) and
> Windows (x64) — installed automatically, nothing to compile. On **musl** (e.g. `node:*-alpine`
> Docker images) there is no prebuilt yet: use a glibc base image such as `node:20-slim`, or
> install the Rust toolchain so the addon builds from source. (A musl prebuild is on the roadmap.)

Ailu's core is **governance**: a run pauses at a human-approval gate and **resumes from
its checkpoint** — deterministically, even across process restarts. Here it is, end to end:

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review")                          // ← the run SUSPENDS here for human approval
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const suspended = await app.run();              // status: "suspended" — checkpointed, waiting
const done = await app.resume(suspended.runId); // status: "completed" — resumes from the checkpoint
```

Add an agent — pick a model with one line; the call runs **in the Rust engine** (no TS
provider client). Keys come from the environment, and a missing one fails loud with the exact
variable to set:

```ts
import { createGraph, model } from "@ailu/graph-sdk";

const app = createGraph({ name: "assistant" })
  .agentNode("reply", { model: model.openai("gpt-4o"), prompt: { system: "Be concise." } })
  .compile();

await app.run({ question: "Summarize this PR." }); // OPENAI_API_KEY; or model.anthropic(), model.fast, …
```

Conditional routing is always a **named predicate**, never an `eval`'d string — what keeps
Ailu's flows safe and inspectable. Channel value types flow through the builder, so handler
state and the result of `run`/`resume` are fully typed with no manual annotation.

### Explore the governed tutorials

Clone the repo to run the tutorials — every one is **offline** (mock LLM, no API key) and
**self-verifying**, so they double as end-to-end tests:

```bash
git clone https://github.com/AI-Adriane/ailu-core.git && cd ailu-engine && pnpm install
pnpm --filter @ailu/graph-sdk example         # governance — suspend/resume with a human gate
pnpm --filter @ailu/graph-sdk example:agent   # an agent routed into an approval gate
pnpm --filter @ailu/graph-sdk example:startup # idea → ship: a governed venture pipeline
```

Full index + walkthroughs: [`packages/graph-sdk/examples/README.md`](packages/graph-sdk/examples/README.md) ·
choosing a model: [`docs-site/docs/recipes/model-packages.md`](docs-site/docs/recipes/model-packages.md).

## What you get from the framework

- **Deterministic, resumable runs** — checkpoint after every node and state
  mutation; resume from the latest checkpoint, including after suspension.
- **Human-in-the-loop gates** — `human-gate` nodes suspend the run and wait for
  approval before continuing.
- **Streaming** — observe `values`, `updates`, `messages` or `debug` events as a
  graph executes, or stream an agent's generation **per token** (`messages` mode,
  or `runCatalogGraph({ streamTokens: true })` on the catalog path).
- **Retrieval** — lexical (BM25/keyword) + semantic retrievers, RRF fusion, and a
  `reranker` that re-scores through a real **cross-encoder** (`bge-reranker-v2-m3`
  via a self-hostable rerank service, `AILU_RERANK_ENDPOINT`) or passes through
  cleanly when none is set.
- **Time-travel, fan-out/`send`, cycles, subgraphs and tool nodes** — the full
  runtime contract, framework-agnostic and Vitest-covered.
- **Multi-agent patterns** — ReAct, supervisor, swarm, plan-execute, reflection, and a governed
  **LLM Council** (`council(...)`: dispatch → members → anonymized peer-review → chair), all as
  checkpointed, audited graphs.
- **Safe by construction** — no `eval` / `new Function` / dynamic `import()` of
  user strings; conditions are named registry predicates; agents cannot approve
  their own outputs; sensitive actions route through approval gates.

## Architecture

This repository **is** the framework. The Rust engine lives in `crates/`, the
TypeScript and Python SDKs and supporting packages in `packages/`.

```
crates/     the Rust execution engine  (OPEN SOURCE)
packages/   the framework              (OPEN SOURCE)
  graph-core       pure data model: GraphDefinition, channels, validation, errors
  graph-runtime    the execution engine: checkpoints, events, suspend/resume, streaming
  graph-sdk        ← you are here: the ergonomic front door
  agents-core      agent patterns (ReAct, plan-execute, reflection, supervisor, swarm, council)
  llm-gateway      the only package allowed to import provider SDKs
  lang-ailu     the Ailu DSL compilers (YAML → graph)
  artifact-store · approval-engine · observability · memory-store · rag-pipeline
  ailu-cli      run and inspect graphs from the terminal
```

> Everything in this repository is the open framework. The SDK is the supported,
> stable surface — import `@ailu/graph-sdk`, not a package's internals.
> **Ailu Studio**, the hosted commercial control plane (visual builder, fleet,
> tracing, evaluation, multi-tenant governance), is a separate product built on top
> of this framework and is not part of this repository.

## Engine: Rust only (no TypeScript fallback)

Graph **execution always runs on the Rust engine** in [`crates/`](crates/), reached from
`@ailu/graph-sdk` through the `@ailu/napi` native addon (an async bridge that
calls back into JS condition/node/tool seams over a ThreadsafeFunction). The addon is
published **prebuilt** for the common platforms and installed automatically with the SDK —
nothing to compile. There is **no TypeScript execution fallback**: if the native engine
genuinely cannot run a graph, the SDK throws `RustEngineRequiredError` rather than silently
degrading (ADR 0016).

The TypeScript engine packages — `graph-runtime`, `agents-core`, `llm-gateway`,
`approval-engine`, `memory-store`, `artifact-store`, `callbacks`, `observability`, `runnable`,
`rag-pipeline`, `lang-ailu`, `graph-ailu` — are therefore **deprecated as execution
engines**. `graph-sdk` (the front door) and `graph-core` (the shared data model + validator)
are **not** deprecated. Import `@ailu/graph-sdk`; do not depend on the engine packages
directly. See [`docs/adr/0003-ts-engine-deprecated-sdk-on-rust.md`](docs/adr/0003-ts-engine-deprecated-sdk-on-rust.md)
and [`docs/adr/0016-rust-only-sdk-no-ts-fallback.md`](docs/adr/0016-rust-only-sdk-no-ts-fallback.md).

## Development

```bash
pnpm install
pnpm build        # turbo run build (respects ^build order)
pnpm test         # vitest across all workspaces
pnpm typecheck
pnpm lint
pnpm rust:check   # cargo fmt + clippy -D warnings + tests, using Cargo.lock

# scope to one package
pnpm --filter @ailu/graph-sdk test
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the working conventions and
`.cursor/rules/*.mdc` for the authoritative per-layer rules.

## License

The framework (`crates/*`, `packages/*`, `plugin/*`) is open source under
Apache-2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE) for details. Ailu
Studio is a separate commercial product and is not part of this repository.
