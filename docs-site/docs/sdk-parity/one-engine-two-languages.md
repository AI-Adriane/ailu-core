---
sidebar_position: 1
title: One engine, many languages
description: The Rust engine parity contract for TypeScript, Python, and the C-ABI SDKs.
---

# One engine, many languages

Ailu is **one Rust engine** with multiple SDK surfaces. This page is the parity contract:
what is guaranteed identical, what is exposed through the shared C ABI, and which pieces still
need idiomatic SDK wrappers around that ABI.

## Shared engine contract

The graph model, the validator, the DSL compiler, the model policy, and the component and
prebuilt catalogs live once, in Rust. SDKs call into that same code:

- A graph that **validates** in one SDK validates identically in the others.
- A DSL document that **compiles** in one SDK compiles the same way in the others.
- `resolve_model` / model-policy decisions are the same given the same environment.
- The component and prebuilt catalogs are the same lists.
- Native component and prebuilt-agent runs execute in Rust, not as per-language rewrites.

There is no second implementation to drift. This is the whole point of the design.

## SDK families

| Family | Languages | Binding |
| --- | --- | --- |
| TypeScript | TypeScript / JavaScript | N-API package with callback-heavy graph execution |
| Python | Python | PyO3 wheel with JSON-in / JSON-out helpers |
| C ABI SDKs | Ruby, PHP, Lua, PowerShell, Go, C, C++, Zig, Swift, Objective-C, Java, Kotlin, Scala, C#, Elixir | `crates/c-api` shared dynamic library |

The C ABI SDKs share the callback-neutral helpers and the callback-capable runtime entry points.
That gives every language the real Rust engine without inventing a runtime bridge per language.

## Feature parity matrix

| Capability | TypeScript | Python | C ABI SDKs |
| --- | --- | --- | --- |
| Engine version | Yes | Yes | Yes |
| Validate `GraphDefinition` JSON | Yes | Yes | Yes |
| Compile graph YAML DSL | Yes | Yes | Yes |
| Resolve model tier / provider override | Yes | Yes | Yes |
| List component catalog | Yes | Yes | Yes |
| List prebuilt agents | Yes | Yes | Yes |
| Run native component | Yes | Yes | Yes |
| Run prebuilt micro-agent | Yes | Yes | Yes |
| Engine run from `EngineSpec` | Yes | Planned | Yes, C ABI |
| Resume / approve / signal / replay | Yes | Planned | Yes, C ABI |
| Host-language custom node handlers | Yes | Planned | Yes, C ABI callbacks |
| Host-language tool handlers | Yes | Planned | Yes, C ABI callbacks |
| Host-language conditional predicates | Yes | Planned | Yes, C ABI callbacks |
| Streaming lifecycle/token events | Yes | Planned | Yes, C ABI callbacks |
| Fluent graph builder | Yes | Planned | Planned SDK ergonomics |
| Checkpointer interface helpers | Yes | Planned | Planned SDK ergonomics over serialized `GraphState` |

The parity target is not optional: every TypeScript capability should either exist in each SDK
or be tracked as an SDK gap. The C ABI now covers both the callback-neutral engine surface and the
runtime callback surface. The remaining work is mostly language ergonomics: builders, typed
helpers, package publishing, and native checkpointer adapters around the serialized run state.

## Callback boundary

Custom node handlers, host tool handlers, conditional predicates, tool approval flows, and
streaming are not simple JSON functions. They require host callbacks, runtime ownership rules,
threading rules, error propagation, and allocator boundaries. TypeScript has that through N-API;
the C ABI now exposes the same runtime shape through `AiluCallbacks`.

At the ABI level, use:

- **TypeScript** for full custom graph execution, streaming, human gates, and checkpointers.
- **C ABI SDKs** for the same runtime shape when a language adapter can provide thread-safe
  callback functions.
- **Python** for validation, DSL compilation, model policy, catalogs, native components, and
  prebuilt micro-agent runs until its PyO3 layer grows the same callback runtime.

## Next

- [TypeScript SDK](./typescript-sdk)
- [Python SDK](./python-sdk)
- [Polyglot C ABI](./polyglot-c-abi)
