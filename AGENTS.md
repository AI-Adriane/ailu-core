# Agent instructions for ailu-core

This file is the single source of repository instructions for Codex, Cursor, and Claude.
The scoped rules in `.cursor/rules/` summarize these instructions for the relevant paths;
keep them consistent with this file. Read [README.md](README.md),
[CONTRIBUTING.md](CONTRIBUTING.md), and the affected code before changing behavior.
Write code documentation and contributor-facing text in English.

## Project and repository map

ailu-core is Ailu's open-source graph execution engine and language bindings, licensed under
Apache-2.0. Rust owns execution; SDKs expose it without implementing another engine.
The supported TypeScript entry point is `@ailu-ai/graph-sdk`, backed by `@ailu-ai/napi`.
There is no TypeScript execution fallback: preserve `RustEngineRequiredError` when the native
engine cannot execute. See [the Rust-only SDK ADR](docs/adr/0016-rust-only-sdk-no-ts-fallback.md).

| Path | Contents |
| --- | --- |
| `crates/` | Cargo workspace (`crates/Cargo.toml`): graph-core, graph-runtime, runtime-bridge, agents-core, llm-gateway, approval-engine, memory, storage, DSL, observability, and CLI crates. `bindings/` provides N-API, `py-bindings/` PyO3, and `c-api/` the C ABI. |
| `packages/` | TypeScript `@ailu-ai/*` workspaces: graph-sdk, graph-core, contracts, model-core and model-specific adapters, CLI, and supporting packages. Legacy execution packages remain in the tree but are deprecated as engines; do not introduce another execution path. |
| `python/` | Python `ailu` wrapper, PyO3 wheel configuration in `pyproject.toml`, and `tests/`. |
| `sdks/` | Thin C ABI bindings for Go, C++, Zig, Swift, Objective-C, C#, JVM languages, Ruby, PHP, Lua, PowerShell, and Elixir; see `sdks/README.md` for binding and memory ownership contracts. |
| `plugin/mcp/` | In-process stdio MCP server over the SDK/native engine, with Vitest tests and a smoke script. |
| `scripts/` | Rust check runner, native binding builds, registry-name reservation helper, and `golden/attestations.ts`. |
| `docs/` | Documentation entry point and published architecture decisions in `docs/adr/`. |
| `docs-site/` | Docusaurus site: pages in `docs/`, navigation in `sidebars.js`, components in `src/`, assets in `static/`, and site scripts. |

## Development commands

Use Node >= 22, pnpm 10.11.0 (`packageManager` in `package.json`), and the Rust toolchain
pinned in `rust-toolchain.toml` (1.96.0, with clippy and rustfmt). No database is required for
the basic engine build and checks.

Run these from the repository root:

| Command | Purpose |
| --- | --- |
| `pnpm install` | Install workspace dependencies. |
| `pnpm build` | Turbo workspace builds, with dependency builds first. |
| `pnpm typecheck` | Turbo workspace type checks, with dependency builds first. |
| `pnpm test` | Turbo workspace tests, with dependency builds first. |
| `pnpm lint` | Turbo workspace lint tasks. |
| `pnpm test:coverage` | Root Vitest coverage across configured package/plugin projects. |
| `pnpm rust:build` | Locked Cargo workspace build. |
| `pnpm rust:fmt` | Cargo formatting check. |
| `pnpm rust:lint` | Locked workspace Clippy, all targets, warnings denied. |
| `pnpm rust:test` | Locked Cargo workspace tests. |
| `pnpm rust:check` | Rust formatting check, Clippy, then tests. |
| `pnpm napi:build` | Build and install the local native addon; required before SDK execution tests in a source checkout. |
| `pnpm py:build` | Build and copy the local PyO3 extension into `python/ailu/`. |
| `pnpm --filter @ailu-ai/graph-sdk test` | Run the SDK's Vitest suite. |
| `pnpm --filter @ailu-ai/plugin-mcp test` | Run MCP Vitest tests and the smoke script. |

`package.json`, `turbo.json`, and `scripts/rust-check.sh` define these gates. The Rust workspace
is under `crates/`, not the repository root; the wrappers select its manifest and locked graph.
For a focused Rust test, use `cargo test --locked --manifest-path crates/Cargo.toml -p ailu-graph-runtime`.

The documentation site is outside the pnpm workspace. From `docs-site/`, run `pnpm install`,
`pnpm start` for live preview, and `pnpm build` to validate MDX and links. Add new pages to the
manual `sidebars.js`. Its current `typecheck` script tolerates errors; use the site build as the
documented docs gate, and do not report that script as strict type-check evidence.

Run the checks relevant to the changed layers and report the exact commands and results.
Distinguish passed checks from checks that were blocked or not run. Never infer release status
from a local build.

## Architecture and runtime invariants

- Keep graph-core pure: data types, schemas, validation, typed errors, and pure utilities only.
  No I/O, provider calls, runtime execution, or dependency on graph-runtime. This applies to
  both `crates/graph-core/` and `packages/graph-core/`.
- Only llm-gateway may import LLM provider SDKs. Route provider calls through the gateway;
  model adapters and callers must not bypass it.
- Import public TypeScript APIs through package roots, never another package's `src/` or
  internal build paths. Export public additions through the package entry point. Keep SDK/FFI
  layers thin and preserve the C ABI ownership contract.
- Execution is deterministic by default, and runs must remain resumable.
- Write a checkpoint after every node completion and every state mutation.
- Emit one lifecycle event for each node transition, in order; preserve run lifecycle events too.
- A human gate suspends execution and emits `run_suspended`. Resume restores the latest
  checkpoint and continues from the suspended position without repeating completed side effects.
- Graph edge conditions are names resolved by a `ConditionRegistry` (or the corresponding
  native registry/callback seam). Registered predicates are pure functions of state; graph
  definitions contain names, never executable expressions or evaluated code.
- Preserve these contracts across retries, failure paths, fan-out, cycles, subgraphs, streaming,
  interrupts, and cancellation. Enforce each node's retry policy.

## Security

- Never use `eval`, `new Function`, or dynamic `import()`/`require()` of user-supplied strings.
- Inject secrets through environment variables only. Never embed credentials in source, graph
  definitions, prompts, artifacts, fixtures, or snapshots; never commit environment secrets.
- Logs and tests must contain no secrets or personal data (PII). Use synthetic, non-sensitive
  fixtures. Do not log full prompt/completion payloads; preserve redaction and `noLog` handling.
- Validate untrusted input at the boundary. Delimit user content from trusted prompt instructions;
  treat model/tool output as untrusted data.
- Enforce declared tool permissions and explicit allow-listing for sensitive capabilities.
  Sensitive actions require an approval gate; an agent must not approve its own output.
- Follow [SECURITY.md](SECURITY.md) for vulnerability reporting.

## Tests and coding conventions

- TypeScript is strict: no `as any`, `@ts-ignore`, or weakened compiler checks. Prefer `unknown`
  and validate external data with Zod before using it.
- Represent fallible TypeScript results as discriminated unions, for example
  `{ success: true; data: T } | { success: false; error: E }`. Use typed errors rather than
  bare generic errors. Preserve Rust's typed `Result` contracts across bindings.
- Put TypeScript tests next to source (`foo.ts` -> `foo.test.ts`); use Vitest, including `.mts`
  tests in the MCP plugin. Rust uses `cargo test`, inline test modules beside implementation
  and crate `tests/` for public integration behavior. Python tests live in `python/tests/`.
- Test observable behavior through public APIs. Mock only I/O boundaries (network, files,
  storage, LLM), never pure functions, registries, validators, or the scheduler under test.
- Cover completion, typed failure, checkpoint/restore, event order and count, human suspension
  and resume, named condition routing, and retry exhaustion when changing execution paths.
  Test the real Rust path for SDK execution behavior.
- Do not add disabled, skipped, ignored, or focused-only tests, or hide failures to make a gate pass.
- Match existing module/import conventions. Prettier uses double quotes, semicolons, no trailing
  commas, and width 100. Reuse existing helpers and keep changes focused.

## Review, versioning, and delivery

Keep PRs small, focused, and reversible; explain the behavior change, why it is needed, and
validation results. Use Conventional Commits and the DCO sign-off required by CONTRIBUTING.md.
Changes to public APIs require maintainer review. Propose structural changes in a public ADR
under `docs/adr/` and obtain review before implementation; this includes runtime invariants,
security-sensitive changes, and large rewrites. Small fixes and docs do not require an ADR.
Only cite ADR files that actually exist in this repository.

Follow [RELEASING.md](RELEASING.md) for version alignment, SDK parity, artifact selection, and
release order. Check current manifests rather than copying version examples. Keep Rust,
TypeScript, and Python capabilities aligned, or document an explicit SDK gap in CHANGELOG.md
as required by the release guide. Do not recursively publish the workspace. Publishing, tagging,
pushing, and merging require explicit authorization; preparing a change does not authorize a release.
