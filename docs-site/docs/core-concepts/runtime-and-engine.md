---
sidebar_position: 5
title: Runtime and engine
description: The Rust engine — the required, Rust-only runtime — and why there is no TypeScript execution fallback.
---

# Runtime and engine

Ailu has **one engine, written in Rust**, and **two SDK surfaces** over it. Understanding
where execution actually happens explains the framework's performance story and its
cross-language guarantees.

## The Rust engine

The graph model, the validator, the DSL compiler, the model policy, and the component and
prebuilt catalogs all live once, in Rust (`crates/`). This is the single source of truth.
There is no parallel TypeScript or Python re-implementation to drift out of sync.

## Two surfaces over it

- **Python** reaches the engine through a [pyo3](https://pyo3.rs) extension
  (`crates/py-bindings`). The wheel ships the compiled engine, so it is **always present** —
  there is no fallback path. `import ailu` *is* the engine.
- **TypeScript** reaches the engine through a [napi-rs](https://napi.rs) addon
  (`crates/bindings`, published as `@ailu-ai/napi`). This addon is a **required
  dependency** of `@ailu-ai/graph-sdk` — installed automatically, never opt-in.

## Rust is the runtime

Ailu runs on the Rust engine. Because `@ailu-ai/napi` is a regular dependency of the SDK,
a single `npm i @ailu-ai/graph-sdk` installs it and the native engine is active:

```ts
import { rustEngineAvailable } from "@ailu-ai/graph-sdk";

console.log(rustEngineAvailable()); // true — the Rust engine is running
```

## No TypeScript execution fallback

Execution is **Rust-only**. The TypeScript engine packages still exist (and a few legacy tests
exercise them), but they are **not an execution fallback**: if the native addon genuinely can't
run a graph, the SDK throws `RustEngineRequiredError` rather than silently degrading (ADR 0016).
The prebuilt addon ships with the SDK and covers the common platforms; on an uncovered target
(e.g. musl/Alpine) you build it from source or use a glibc base image — you never get a quiet
TS-engine run with different guarantees.

## The bridge

The native boundary is narrow and JSON-shaped. The addon exposes a handful of entry points —
graph validation, DSL compilation, the model policy, and the fully-Rust run paths — and three
callbacks let the engine call back into the host for node handlers, condition predicates, and
event emission. The bridge is detailed in
[Architecture → the native bridge](/docs/architecture/napi-bridge).

## Practical implications

- **In TypeScript**, the Rust addon is a required dependency — it's there after a normal
  install. Don't branch your application logic on `rustEngineAvailable()` for *correctness* —
  only, at most, for diagnostics.
- **In Python**, the engine is always there too; the wheel ships it, so there is nothing to
  install separately.
- **In C-ABI SDKs**, the wrapper loads the shared Rust dynamic library and exposes the same
  JSON/YAML helpers plus the callback-capable runtime ABI.
- **Across SDKs**, a graph that validates or compiles one way validates and compiles the same
  way in the others, because the same Rust code answers in every binding.

## Next

- [SDK parity: one engine, many languages](/docs/sdk-parity/one-engine-two-languages)
- [Architecture overview](/docs/architecture/overview)
