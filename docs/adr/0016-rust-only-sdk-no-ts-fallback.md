# ADR 0016 — Rust-only SDK: remove the TypeScript engine fallback

- Status: Accepted (implemented)
- Date: 2026-06-22
- Deciders: Mathieu (owner)
- Builds on: [ADR 0002](0002-rust-engine-migration.md) (engine in Rust), [ADR 0003](0003-ts-engine-deprecated-sdk-on-rust.md) (deprecate the TS engine; run the SDK on Rust)

## Context

ADR 0003 deprecated the in-process TypeScript engine and made the Rust engine (via
`@ailu/napi`) the canonical runtime, while keeping the TS `GraphRuntime` as a silent
fallback when the native addon was absent. That fallback turned out to be a liability:

- It let a graph **silently** run on a second, divergent execution path — the exact "two
  engines, subtle differences" risk the one-Rust-engine architecture exists to remove.
- Several behaviours only ever worked on the TS path (an `ApprovalEngine`-backed agent node,
  a JS handler returning a routing `Command { goto }`, a `requiresApproval` tool node that
  suspends), so "it works in dev" could mean "it works only on the fallback."
- It contradicted the SDK's intended role: a **thin surface over the Rust engine**, not an
  engine in its own right.

## Decision

The SDK is **Rust-only**. `CompiledGraph` throws `RustEngineRequiredError` at compile time
when the native engine cannot run the graph (napi absent, `AILU_SDK_ENGINE=ts`, or a
TS-only feature is used) instead of degrading to the TypeScript runtime. The TS execution
branches and their orphaned helpers are removed; `@ailu/napi` is a hard runtime
requirement everywhere a graph runs.

## Consequences

- **BREAKING.** Graphs require napi at runtime. CI/test environments must build the native
  addon before running graph-sdk tests (the public CI builds it in the coverage + unit jobs).
- TS-only features must move to the Rust-supported equivalents: channel-based routing
  (conditional edges) instead of `Command { goto }`; channel/catalog-seam approvals
  (`runCatalogGraph` + `ApprovalEngine`) instead of the TS `approvalEngine` agent option.
- The `engine` escape-hatch getter + the in-process `GraphRuntime` remain constructed for
  time-travel/manual use but no longer execute a run. Fully excising `GraphRuntime` and the
  residual TS `llm`/`approvalEngine` agent options is a follow-up (public-API removal).
- Tests that pinned `AILU_SDK_ENGINE=ts` or asserted TS-vs-Rust fidelity were removed; the
  structural contract is covered on Rust by `rust-engine.test.ts` + the catalog-seam tests.

## Reserves

A hard napi requirement raises the floor for first-run/CI. Accepted: it is the price of a
single, trustworthy execution path. `RustEngineRequiredError` states the remedy
(`scripts/build-napi.sh` / install `@ailu/napi`).
