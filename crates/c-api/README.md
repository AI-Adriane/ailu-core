# ailu-c-api

Stable C ABI over the Ailu Rust engine for thin polyglot SDKs.

This crate is the common ABI for SDKs outside TypeScript/Python. It exports both
the callback-neutral JSON/YAML helpers and the callback-capable runtime entry
points:

- `ailu_engine_version() -> char*`
- `ailu_validate_graph_json(const char*) -> AiluResult`
- `ailu_compile_graph_yaml_json(const char*) -> AiluResult`
- `ailu_available_providers_json() -> AiluResult`
- `ailu_resolve_model_json(const char*, const char*, const char*) -> AiluResult`
- `ailu_list_components_json() -> AiluResult`
- `ailu_list_prebuilt_json() -> AiluResult`
- `ailu_run_component_json(const char*, const char*, const char*) -> AiluResult`
- `ailu_run_prebuilt_json(const char*, const char*, const char*) -> AiluResult`
- `ailu_engine_run_json(const char*, AiluCallbacks) -> AiluResult`
- `ailu_engine_resume_json(const char*, AiluCallbacks) -> AiluResult`
- `ailu_engine_approve_and_resume_json(const char*, AiluCallbacks) -> AiluResult`
- `ailu_engine_signal_json(const char*, const char*, const char*, AiluCallbacks) -> AiluResult`
- `ailu_engine_replay_json(const char*, const char*, AiluCallbacks) -> AiluResult`
- `ailu_string_free(char*)`
- `ailu_result_free(AiluResult)`

All returned strings are owned by the caller and must be freed with one of the
free functions. See `include/ailu.h` for the C contract.

Runtime string callbacks use `int callback(payload, user_data, &value, &error)`.
Rust copies callback `value` or `error` immediately before returning to the
engine; the host owns those callback pointers and only needs to keep them valid
for the duration of the callback call. Callbacks may be invoked from runtime
worker threads, so host SDK adapters must make their callback storage
thread-safe.

```bash
cargo build --locked --manifest-path crates/Cargo.toml -p ailu-c-api
```

The produced library is platform-specific:

- macOS: `crates/target/debug/libailu_c_api.dylib`
- Linux: `crates/target/debug/libailu_c_api.so`
- Windows: `crates/target/debug/ailu_c_api.dll`

Higher-level SDKs should wrap this ABI with native types and keep the engine
boundary as JSON in / JSON out.
