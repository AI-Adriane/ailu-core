# ADR 0020 — One Rust engine, thin polyglot SDKs (napi + pyo3)

- Status: Accepted (implemented for TS + Python; other languages designed-for, not shipped)
- Date: 2026-06-22
- Deciders: Mathieu (owner)
- Builds on: [ADR 0002](0002-rust-engine-migration.md) (engine in Rust), [ADR 0016](0016-rust-only-sdk-no-ts-fallback.md) (SDK runs only on Rust)

## Context

The graph model, validator, DSL compiler and execution live once in Rust. Each language SDK
must run *that* engine — not reimplement it — so a graph that validates/runs in one language
behaves identically in another. The question is the binding seam.

## Decision

Expose the Rust engine through **thin per-language bindings over one shared core**:
- **TypeScript** via **napi-rs** (`@ailu-ai/napi`): the `crates/bindings` addon, loaded by
  `@ailu-ai/graph-sdk`. The seam serializes a graph to an `EngineSpec` and runs it on Rust;
  JS node handlers + conditions round-trip through napi callbacks (`on_node` / `on_condition`).
- **Python** via **pyo3/maturin** (`crates/py-bindings`): the `ailu` wheel ships the same
  engine (one `cp39-abi3` wheel per platform covers 3.9+).

The SDKs are surfaces, not engines — they hold ergonomics (builder, types, prompt resolution),
never a second execution path (ADR 0016). Go/Java/PHP/.NET/Ruby are *designed for* by this seam
but not shipped.

## Consequences

- True cross-language parity: the validator/runtime/DSL are the same bytes behind every SDK.
- A new language SDK is a binding + a thin surface, not a port of the engine — tractable.
- Cost: every engine change requires a napi (and wheel) rebuild + publish; the bindings crate
  is on the release path. The JS↔Rust callback seam awaits handler promises (Phase F), so async
  JS node handlers/tools round-trip faithfully.

## Reserves

The napi callback boundary adds a Rust↔JS crossing per JS node (measured ~µs; negligible vs an
LLM call, visible only on orchestration-heavy graphs — see the benchmarks). Polyglot beyond
TS/Python is unproven until a third SDK is actually shipped.
