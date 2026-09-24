# ADR 0003 — Deprecate the TypeScript engine; run @ailu/graph-sdk on Rust

- Status: Accepted
- Date: 2026-06-11
- Builds on: [ADR 0002](0002-migrate-engine-to-rust.md)

## Context

ADR 0002 decided to migrate the open-source engine from TypeScript to Rust
incrementally, bottom-up, with the TS engine staying in production until each Rust
crate reached parity and was adopted. That migration has now progressed far enough to
flip the consumer: the Rust crates under `crates/` cover the model and the executor
(`graph-core`, `graph-runtime`), the agent patterns (`agents-core`), the provider
gateway (`llm-gateway`), and the rest of the engine surface, and `@ailu/graph-sdk`
has been re-plumbed to execute on them through a native addon (`@ailu/napi`).

With the SDK able to run on Rust, the TypeScript engine packages are no longer the
intended execution path. We need to record that the TS engine is **deprecated as the
execution engine**, while being explicit about what is *kept* and why nothing breaks.

## Decision

The TypeScript engine packages are **deprecated in their execution role** and retained
only as a fallback. `@ailu/graph-sdk` remains the supported front door and runs on
the Rust engine when the native addon is present, falling back to the deprecated TS
engine when it is absent.

### What is deprecated

These packages get a `[DEPRECATED — …]` prefix on their `package.json` description and
a top-of-file `@deprecated` JSDoc banner on `src/index.ts`, each pointing at its Rust
crate equivalent:

`graph-runtime`, `agents-core`, `llm-gateway`, `approval-engine`, `memory-store`,
`artifact-store`, `callbacks`, `observability`, `runnable`, `rag-pipeline`,
`lang-ailu`, `graph-ailu`.

The deprecation is **signalling and documentation only**: no code is deleted, no
runtime behaviour changes, and `apps/*` (the Studio still imports the TS engine and
must keep compiling) are untouched.

### What is explicitly retained (not deprecated)

- **`@ailu/graph-sdk`** — the kept, supported front door. Its public API is
  unchanged; whether it runs on Rust or TS is an internal detail behind a read-only
  `usesRustEngine` getter and the exported `rustEngineAvailable()`.
- **`@ailu/graph-core`** — the pure data model + Zod validator. It is the shared
  foundation for both the TS fallback and the wire contract with the Rust engine
  (serde camelCase ↔ TS types), so it stays first-class.
- `@ailu/contracts`, `@ailu/db`, `@ailu/config`, `@ailu/ui` — not engine
  execution packages; unaffected.

### Architecture: the napi async + ThreadsafeFunction bridge

`@ailu/graph-sdk` reaches the Rust engine through the optional `@ailu/napi`
native addon, loaded with `createRequire` so its absence is a clean miss (mirroring the
existing `rust-validator.ts` pattern):

- The SDK calls async entry points `engine_run` / `engine_resume` /
  `engine_approve_and_resume` on the native module.
- The Rust engine calls back into JS over a **ThreadsafeFunction** for the seams that
  must stay in JS: named **condition** predicates (returning a boolean), and the
  **node / tool** execute seams (returning a JSON string).
- Boundary contract (verified against napi 2.16): the Rust seam resolves each JS
  callback's **synchronous** return value — it does not await a returned Promise (a
  returned thenable aborts the process). Every callback returns synchronously: a JSON
  string for node/tool seams, a boolean for conditions.
  > **SUPERSEDED by [ADR 0004](0004-approval-enforcement-on-rust-golden-path.md).**
  > This synchronous description is out of date. As of Phase F the seam is
  > **asynchronous**: the Rust side awaits the JS callback's returned `Promise` via
  > `call_async::<Promise<T>>(..).await?.await?`. ADR 0004 is the authority for the
  > boundary contract.
- Agent config is threaded across the boundary as a serializable `RustAgentConfig`
  (provider, model, resolved system-prompt string, tool names, max iterations,
  approval settings, output channel, per-tool JS execute bindings) via
  `toRustAgentConfig(...)`, captured per `agentNode()` in the builder.

### Fallback rule

> Run on **Rust if the `@ailu/napi` addon is present; otherwise fall back to the
> (deprecated) TypeScript engine.**

When the addon is absent, `rustEngineAvailable()` returns `false` and `CompiledGraph`
executes on the deprecated `@ailu/graph-runtime`. This keeps `main` building and the
Studio working with or without a built native addon.

## Consequences

- **No breakage:** deprecation is metadata + JSDoc only. `pnpm typecheck`, `pnpm test`,
  and `pnpm lint` stay green; the Studio (which imports the TS engine) still compiles.
- **Clear direction for consumers:** the IDE surfaces the `@deprecated` banners and
  npm shows the `[DEPRECATED — …]` description, steering new code to
  `@ailu/graph-sdk` and the Rust crates instead of the TS engine internals.
- **Two engines coexist** behind one SDK API until the native addon is universally
  built; the TS engine remains a working safety net rather than dead code.
- **Future:** once the Rust addon ships everywhere by default, a later ADR can decide
  whether to remove the TS engine packages entirely. This ADR does not delete them.
