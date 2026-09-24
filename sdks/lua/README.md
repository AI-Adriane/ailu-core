# Ailu Lua SDK

Minimal LuaJIT FFI wrapper over `ailu-c-api`.

Build the C ABI library first:

```bash
cargo build --locked --manifest-path crates/Cargo.toml -p ailu-c-api
```

Then point LuaJIT at the library:

```bash
AILU_C_API_LIB=crates/target/debug/libailu_c_api.dylib luajit -e '
local ailu = require("sdks.lua.ailu")
print(ailu.engine_version())
print(ailu.list_components_json())
'
```

On Linux use `libailu_c_api.so`; on Windows use `ailu_c_api.dll`.
