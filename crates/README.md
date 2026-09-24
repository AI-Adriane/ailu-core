# Ailu — Rust engine (migration in progress)

This workspace is the staged Rust port of the open-source engine (the TypeScript
`packages/*`). It exists alongside the TS engine, not as a big-bang replacement —
each crate lands compiling and tested before the next, and the TS engine keeps
working throughout. See `docs/adr/0002-migrate-engine-to-rust.md` for the plan and
rationale.

## Status

The workspace is checked with `cargo 1.96.0` (fmt + clippy `-D warnings` +
`cargo test --locked`). The Rust engine is consumed through the napi bindings,
the `ailu` CLI binary, an MCP server, the Python SDK (pyo3), and now a stable
C ABI for the next polyglot SDKs. The TypeScript SDK runs through the Rust
engine via `@ailu-ai/napi` with no TypeScript runtime fallback.

| Crate | Mirrors | Status |
| --- | --- | --- |
| `ailu-graph-core` | `@ailu-ai/graph-core` | ✅ **Done** — ids, types, errors, `validate_graph`. 8 tests. |
| `ailu-graph-runtime` | `@ailu-ai/graph-runtime` | ✅ **Async, near-parity** — async node handlers (tokio), start/resume/suspend, default + conditional edges, reducers, checkpointer, event bus, DynamicInterrupt + `update_state`, fan-out → join, recursion limit, **retries + `run_failed`, time-travel `replay_from` (fork), live event observers**. 13 tests. Deferred: streaming modes, subgraphs. |
| `ailu-approval-engine` | `@ailu-ai/approval-engine` | ✅ **Done** — engine (no self-approval) + Ed25519 attestation (canonicalise / sign / verify / chain). 9 tests. |
| `ailu-llm-gateway` | `@ailu-ai/llm-gateway` | ✅ **Gateway + real Anthropic adapter** — async `LlmGateway` / `LlmProviderAdapter`, `DefaultLlmGateway`, `MockAdapter`, **`AnthropicAdapter` (reqwest/rustls, testable port seam, cache-aware prefix, tool_use → tool_calls)**. 14 tests. Deferred: structured content blocks, streaming. |
| `ailu-agents-core` | `@ailu-ai/agents-core` | ✅ **ReAct + patterns** — ReAct loop (native tool calls + `FINAL:`/`ACTION:`), no-self-approval tool registry, `agent_node_handler` (suspend-for-approval via `__approvedTools`), **plan-execute, reflection, supervisor, working-memory**. 26 tests. Deferred: swarm handoff, scratchpad. |
| `ailu-graph-ailu` | `@ailu-ai/graph-ailu` | ✅ **DSL compiler** — YAML → AST → DSL validation → transform → structural gate; byte-equivalent JSON to the TS compiler. 14 tests. |
| `ailu-lang-ailu` | `@ailu-ai/lang-ailu` | ✅ **Prompt/agent/chain DSL compiler** — same pipeline as graph-ailu, byte-equivalent JSON to the TS compiler. 21 tests. |
| `ailu-memory-store` | `@ailu-ai/memory-store` | ✅ **BaseStore + InMemoryStore** — namespaced get/put/delete/search/list, insertion-ordered. 6 tests. PgStore deferred. |
| `ailu-artifact-store` | `@ailu-ai/artifact-store` | ✅ **Versioned store** — `Artifact`/`ArtifactRef`, write increments version, read/list. 12 tests. PgArtifactStore deferred. |
| `ailu-callbacks` | `@ailu-ai/callbacks` | ✅ **CallbackManager** — 15-variant `CallbackEvent`, async dispatch with handler isolation, child managers, built-in handlers. 11 tests. |
| `ailu-observability` | `@ailu-ai/observability` | ✅ **Tracer + metrics + bus** — spans (start/child/end + duration), counter/gauge/histogram, event bus fan-out/unsubscribe. 11 tests. |
| `ailu-runnable` | `@ailu-ai/runnable` | ✅ **Composable Runnable** — async `Runnable<I,O>`, lambda/passthrough/sequence (`then`)/parallel (join_all, keyed). 9 tests. Fluent `.pipe()` → explicit constructors (documented). |
| `ailu-config` | `@ailu-ai/config` | ✅ **Env + feature flags** — pure `parse_env(map)` (aggregate validation errors) + `get_env()` (OnceLock), typed flags. 13 tests. |
| `ailu-rag-pipeline` | `@ailu-ai/rag-pipeline` | ✅ **RAG (mock/in-memory)** — Embedder/Splitter/VectorStore/Retriever/Reranker seams, deterministic MockEmbedder, cosine top-k, end-to-end pipeline. 23 tests. Deferred: real embeddings/loaders. |
| `ailu-napi` | (bindings) | ✅ **Boundary live** — `validateGraphJson`, `compileGraphYamlJson`, `engineVersion` callable from Node; the SDK's `safeCompile()` uses it. See `bindings/README.md`. |
| `ailu-runtime-bridge` | (shared runtime bridge) | ✅ **Callback runtime bridge** — shared `EngineSpec` runner used by N-API and C ABI: run/resume/approve/signal/replay, host node/tool callbacks, conditions, lifecycle/token events. |
| `ailu-sdk-core` | (shared SDK core) | ✅ **JSON core** — callback-neutral SDK surface shared by Python and the C ABI: validate/compile, model policy, catalogs, native components, prebuilt agents. |
| `ailu-c-api` | (polyglot bindings) | ✅ **C ABI foundation** — stable `AiluResult` memory ownership, JSON helpers, and callback-capable runtime entry points for Ruby/PHP/Lua/PowerShell/Go/C/C++/Zig/Swift/ObjC/JVM/.NET/BEAM SDKs. |
| `ailu-cli` | `@ailu-ai/cli` | ✅ **CLI binary** — `ailu compile/validate/run/inspect/--help`; run drives the async runtime (suspends cleanly at human gates), event journal to stderr. 13 tests. |
| `ailu-py` | (Python SDK ailu) | ✅ **2nd SDK (pyo3)** — `python/ailu` wraps the Rust core (`validate_graph`, `compile_graph_yaml`, `engine_version`); abi3, builds with cargo. Proves multi-language SDKs over one engine. 5 py tests. |
| Claude Code plugin | `plugin/` | ✅ **Plugin** — skill (authoring graphs), slash commands (`/ailu:compile|validate|run|new` over the CLI), MCP server exposing the Rust engine (`validate_graph`, `compile_graph_yaml` via `@ailu-ai/napi`). MCP smoke verified. |
| `graph-sdk` · `contracts` | (TS-facing surfaces) | Stay TS (SDK facade over napi; contracts is the API↔Studio DTO boundary). |

## Build & test

```bash
# Install rustup if needed:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# From the repository root. rust-toolchain.toml pins the exact Rust version.
pnpm rust:check     # fmt + clippy -D warnings + tests
pnpm rust:build     # compile the full Rust workspace
pnpm rust:fmt       # formatting only
pnpm rust:lint      # clippy only
pnpm rust:test      # tests only
```

## Design notes

- **Wire-compatible:** types serialize to the same camelCase JSON as the TS model
  (`type`, `entryNodeId`, `retryPolicy`, `human-gate`, …), so a Rust engine and the
  TS control plane / SDK can exchange definitions and state during the migration.
- **`#![forbid(unsafe_code)]`** in engine crates. `ailu-c-api` is the deliberate
  exception: it uses a narrow, documented unsafe boundary for C pointers and
  allocator ownership.
- Validation returns a `Vec<ValidationError>` (every problem at once), matching
  `validateGraph`.
