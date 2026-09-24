# ailu-napi — Node bindings for the Rust engine

napi-rs bindings that expose the Rust engine to Node, so the TypeScript SDK and
control plane can call the Rust core during the migration (ADR 0002) without a flag
day. JSON in / JSON out keeps the boundary trivial.

Published to the pnpm workspace as `@ailu-ai/napi` (see `pnpm-workspace.yaml`).

## Exposed

- `engineVersion(): string`
- `validateGraphJson(definitionJson: string): string` — returns a JSON array of
  validation errors (`[]` when sound). The drop-in replacement for the TS
  `validateGraph` (same `GraphDefinition` wire shape, same error codes).

## Dev flow (the one to use day to day)

From the repo root:

```bash
pnpm napi:build        # runs scripts/build-napi.sh
```

The script runs `cargo build -p ailu-napi`, detects the platform
(`.dylib` on macOS, `.so` on Linux, `.dll` otherwise) and copies the cdylib to
`crates/bindings/ailu_napi.node` — exactly the file the handwritten `index.js`
requires. `index.js` and `index.d.ts` are **handwritten and checked in**; the dev
flow never regenerates them.

The script (and the root `rust:*` scripts) assume `cargo` is on `PATH`. rustup's
installer arranges that for normal login shells; the script additionally sources
`"$HOME/.cargo/env"` itself if `cargo` is missing, so it also works from bare
non-login shells.

Smoke test:

```bash
node crates/bindings/smoke.cjs
# engineVersion: 0.0.1
# valid → []
# dangling edge → [{"code":"INVALID_EDGE_REFERENCE",...}]
```

## Release flow (`@napi-rs/cli` — for publish pipelines, not dev)

`package.json` carries a `napi` config block (`"name": "ailu_napi"`, matching
the binary basename) and two scripts, named so turbo ignores them (turbo only
picks up `build`/`test`/`lint`/`typecheck`):

```bash
pnpm --filter @ailu-ai/napi run build:napi          # napi build --platform --release
pnpm --filter @ailu-ai/napi run build:napi:debug    # napi build --platform
```

`napi build --platform` compiles the crate and emits a platform-suffixed binary
(e.g. `ailu_napi.darwin-arm64.node`) plus **regenerated** `index.js`/`index.d.ts`:
the generated loader switch-cases over `process.platform`/`process.arch` and
requires the suffixed binary (or an `@ailu-ai/napi-<platform>` sub-package), which
is what you want when publishing prebuilds for many targets from CI.

**Why dev does not use it:** the generated loader never looks at
`ailu_napi.node`, so it silently ignores what `scripts/build-napi.sh` produces —
on a fresh machine, `pnpm napi:build` would build a binary the loader cannot find.
The CLI path was verified locally (build succeeds, generated loader loads, SDK
example and tests pass), but the handwritten loader is kept as the committed state
so the dev script stays the single source of truth. A real release pipeline should
run `build:napi` per target and ship the generated loader in the published package
instead.

## SDK integration

`packages/graph-sdk/src/rust-validator.ts` loads `@ailu-ai/napi` **optionally**:
if the module (or its `.node` binary) is absent, the require throws, the SDK
catches it and falls back to the pure-TypeScript `validateGraph` from
`@ailu-ai/graph-core`. Nothing in the SDK hard-depends on the native addon.

```bash
pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/rust-validation.ts
# Rust validator active: true     (with the .node present; false → TS fallback)
```

## CI

`.github/workflows/ci.yml` (ready-to-activate; the repo is not under git yet) runs
a `node` lane (pnpm install, typecheck, test, lint) and a `rust` lane
(`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
inside `crates/`). The same gates are wired locally as root scripts:
`pnpm rust:fmt`, `pnpm rust:lint`, `pnpm rust:test`, `pnpm napi:build`.
