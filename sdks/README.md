# Ailu polyglot SDKs

Ailu SDKs are thin surfaces over one Rust engine. The TypeScript SDK keeps its
N-API bridge for Node packaging and async JS callback ergonomics. Python uses
PyO3 for wheels. The remaining SDKs start from the stable C ABI in `crates/c-api`,
including the callback-capable runtime entry points.

## Binding strategy

| Family | Languages | Default binding |
| --- | --- | --- |
| Scripting | JavaScript, TypeScript | Existing N-API bridge (`crates/bindings`) |
| Scripting | Python | Existing PyO3 wheel (`crates/py-bindings`) |
| Scripting | Ruby, PHP, Lua, PowerShell | C ABI (`crates/c-api`) |
| Systems | C, C++, Zig, Swift, Objective-C | C ABI (`crates/c-api`) |
| Systems | Go | C ABI through cgo |
| Systems | Rust | Direct Rust crates |
| JVM / .NET | Java, Kotlin, Scala, C# | C ABI through JNA/JNI or P/Invoke |
| BEAM | Elixir | C ABI through a small NIF or port driver |

## Stable surface

The callback-neutral contract is:

- `engine_version`
- `validate_graph_json`
- `compile_graph_yaml_json`
- `available_providers_json`
- `resolve_model_json`
- `list_components_json`
- `list_prebuilt_json`
- `run_component_json`
- `run_prebuilt_json`

This gives every SDK the same validator, DSL compiler, model policy, catalogs,
native component runs, and prebuilt-agent runs immediately, without duplicating
the engine.

Saved (catalog) graphs and their approvals (ADR 0045 D3) are decided by the engine
too: `spec_from_catalog_json` builds the `EngineSpec` of a graph whose nodes carry
agent / component settings, and `catalog_approval_plan_json`,
`catalog_approvals_to_check_json` and `catalog_resume_problems_json` decide what a
suspended run files with the host's approval store and whether a resume may go on.
No wrapper calls them yet.

The callback-capable runtime contract is:

- `engine_run_json`
- `engine_resume_json`
- `engine_approve_and_resume_json`
- `engine_signal_json`
- `engine_replay_json`

These functions consume the same `EngineSpec` JSON used by the TypeScript N-API
bridge and call host callbacks for custom nodes, host-backed tools, conditional
predicates, and lifecycle/token events.

Cancellation (ADR 0044) goes through versioned entry points (ADR 0045 D2.4):
`engine_run_json_v2`, `engine_resume_json_v2`, `engine_approve_and_resume_json_v2`
and `engine_signal_json_v2` take a pointer to an `AiluCallbacksV2` — the
`AiluCallbacks` fields plus `is_cancelled`, polled at every node boundary — with
its `struct_size` set. The original entry points keep their by-value
`AiluCallbacks`, so a wrapper built against an earlier release keeps working. The
C++ wrapper exposes the `_v2` functions; the other wrappers still call the
original entry points, so a run started from them cannot be cancelled yet: each
needs `AiluCallbacksV2` declared in its FFI layer (cgo, JNA, P/Invoke, FFI cdef,
NIF, …).

## Memory contract

Every ABI function that returns a string returns an owned UTF-8 C string. SDKs
must copy it into their native string type, then call `ailu_string_free` or
`ailu_result_free`.

Never free Ailu-owned strings with a host allocator.

Callback results are different: string callbacks return an integer status and
fill borrowed `value` / `error` output pointers owned by the host. The C ABI
copies them during the callback call and never frees them. Callback adapters must
keep those pointers valid until the callback returns and must be safe to invoke
from runtime worker threads.

## Layout

- `lua/`, `ruby/`, `php/`, and `powershell/` cover the remaining scripting surfaces.
- `go/`, `cpp/`, `zig/`, `swift/`, and `objc/` cover systems-language consumers.
- `csharp/` covers .NET; `jvm/` exposes Java plus Kotlin/Scala delegates.
- `elixir/` provides the BEAM entry point through a small C NIF.
- Future SDK packages should add native builders on top of these JSON methods,
  not reimplement engine behavior.
