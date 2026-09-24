---
sidebar_position: 4
title: Polyglot C ABI
description: The stable C ABI used to start SDKs beyond TypeScript and Python.
---

# Polyglot C ABI

The polyglot SDKs use one shared C ABI over the Rust engine. The ABI lives in
`crates/c-api`, and each SDK wraps it with native language ergonomics instead of
reimplementing engine behavior.

## Supported ABI surface

The callback-neutral contract is JSON/YAML in and JSON out:

| C ABI function | SDK capability |
| --- | --- |
| `ailu_engine_version` | read the bound Rust engine version |
| `ailu_validate_graph_json` | validate a `GraphDefinition` JSON payload |
| `ailu_compile_graph_yaml_json` | compile Ailu graph YAML DSL |
| `ailu_available_providers_json` | list providers enabled by environment credentials |
| `ailu_resolve_model_json` | resolve a model tier and optional override |
| `ailu_list_components_json` | list native component kinds |
| `ailu_list_prebuilt_json` | list prebuilt micro-agents |
| `ailu_run_component_json` | run one native component in Rust |
| `ailu_run_prebuilt_json` | run one prebuilt micro-agent in Rust |
| `ailu_engine_run_json` | start a callback-capable graph run from `EngineSpec` |
| `ailu_engine_resume_json` | resume from serialized `GraphState` |
| `ailu_engine_approve_and_resume_json` | validate approved tools, then resume |
| `ailu_engine_signal_json` | deliver an external signal, then resume |
| `ailu_engine_replay_json` | replay a recorded run from a checkpoint |

SDKs convert native values at their edge, call the ABI, copy the returned UTF-8 string, then
free it with `ailu_result_free` or `ailu_string_free`.

The runtime functions take `AiluCallbacks`:

| Callback | Purpose |
| --- | --- |
| `on_node(payloadJson, userData, &value, &error) -> int` | custom node handlers and host-backed tool execution |
| `on_condition(payloadJson, userData, &value, &error) -> int` | named conditional predicates |
| `on_event(payloadJson, userData)` | lifecycle events and token deltas |

Callback return strings are borrowed from the host, not freed by Rust. String callbacks return an
integer status code and fill either `value` or `error`; the engine copies that pointer during the
callback call, so the host only needs to keep it valid until that callback returns. Runtime
callbacks may run on worker threads; SDK adapters must make their callback storage thread-safe.

## SDK folders

| Language | Folder | Notes |
| --- | --- | --- |
| Ruby | `sdks/ruby` | Uses the `ffi` gem |
| PHP | `sdks/php` | Uses PHP FFI |
| Lua | `sdks/lua` | Uses LuaJIT FFI |
| PowerShell | `sdks/powershell` | Uses embedded P/Invoke |
| Go | `sdks/go` | Uses cgo |
| C | `crates/c-api/include/ailu.h` | Direct ABI header |
| C++ | `sdks/cpp` | Header-only RAII wrapper |
| Zig | `sdks/zig` | Imports `ailu.h` with `@cImport` |
| Swift | `sdks/swift` | SwiftPM package over a C target |
| Objective-C | `sdks/objc` | Foundation wrapper over `ailu.h` |
| Java | `sdks/jvm/java` | Uses JNA |
| Kotlin | `sdks/jvm/kotlin` | Thin wrapper over the Java JNA binding |
| Scala | `sdks/jvm/scala` | Thin wrapper over the Java JNA binding |
| C# | `sdks/csharp` | Uses P/Invoke |
| Elixir | `sdks/elixir` | Uses a small NIF |

## Build the native library

```bash
cargo build --locked --manifest-path crates/Cargo.toml -p ailu-c-api
```

On macOS this produces:

```text
crates/target/debug/libailu_c_api.dylib
```

Most wrappers can be pointed at that library with:

```bash
export AILU_C_API_LIB="$PWD/crates/target/debug/libailu_c_api.dylib"
```

Some toolchains also need the library directory in their native search path:

```bash
export DYLD_LIBRARY_PATH="$PWD/crates/target/debug:$DYLD_LIBRARY_PATH"
```

## JVM dependency

The JVM SDK uses JNA. Add it through Maven or Gradle:

```xml
<dependency>
  <groupId>net.java.dev.jna</groupId>
  <artifactId>jna</artifactId>
  <version>5.16.0</version>
</dependency>
```

Recent JDKs print a native-access warning when JNA calls `System.load`. For local development
that warning is harmless. To silence it explicitly, pass:

```bash
--enable-native-access=ALL-UNNAMED
```

## Why C

Ruby, PHP, Lua, PowerShell, Go, C, C++, Zig, Swift, Objective-C, Java/Kotlin/Scala, C#, and
Elixir all have mature ways to load a C library. A single C ABI prevents the engine from being
rewritten per language.

TypeScript remains on N-API because it already has callback-heavy graph execution. Python
remains on PyO3 for wheel ergonomics. The C ABI is the default for the other SDK surfaces,
including callback-capable graph execution.

## Current scope

The current ABI covers validation, DSL compilation, model policy, catalogs, native component
runs, prebuilt-agent runs, and callback-capable graph execution from the same `EngineSpec` used
by the TypeScript N-API bridge. Language SDKs can now layer native builders and typed helpers on
top of this ABI instead of needing a new Rust bridge per language.

See [one engine, many languages](./one-engine-two-languages) for the parity matrix.
