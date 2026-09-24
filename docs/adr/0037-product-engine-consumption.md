# ADR 0037 — How the product consumes the engine (graph-sdk as the door + a 4-package published residual)

- Status: **Accepted** (owner-confirmed; executing). **Step 1 shipped** — the D2 + D3 graph-sdk re-exports + the search/memory-store inlining (this PR). Remaining (release.yml += contracts/config, the `v1.3.0` publish, the product repoint, D4 `compileGraphYaml`) follow per the sequencing below. Widens the public governance/storage surface of `@ailu/graph-sdk` + sets the open-core publish boundary (mandatory-review items, signed off).
- Forced by: [ADR 0036](0036-a2a-agent-interop.md) phase-14 prerequisite (the A2A adapter in `product/apps/api` needs the `1.3.0` graph-sdk surface — `explainRun` / `signal` / `run` / `resume` / `approveAndResume`, all present at `1.3.0`). The product currently resolves `@ailu/*` via `workspace:*` against a **stale** `engine/` subtree (`1.1.1`), so phase 14 cannot build until consumption is reconciled.
- Builds on: the open-core line (engine OSS / Studio commercial), [repo topology: public `ailu-engine` is the engine source]. graph-sdk is built by **tsup**, which **inlines** the engine packages (`graph-core`, `graph-runtime`, `agents-core`, `llm-gateway`, `artifact-store`, `approval-engine`) via a workspace alias; `napi`, `config`, and `contracts` are kept **external** by design.

## Context

The private monorepo's pnpm workspace globs `engine/packages/*` + `product/packages/*` + `product/apps/*`. `product/apps/api` (the NestJS control plane) imports ~13 engine packages as `workspace:*`, resolved against the **stale local `engine/`** copy. The published npm surface today is only `graph-sdk` (a tsup bundle) + `model-core` + `napi` + `cli` + the Python wheels — **not** the other engine packages. So "repoint the product to the published `1.3.0`" does not work as-is: most of what the product imports is unpublished.

A decoupling audit (per-package usage mapping of `product/apps/*` + `product/packages/*` against the graph-sdk public surface) established the governing fact: **graph-sdk already inlines the engine source**, so most "the product imports an unpublished package" cases are resolved by **additive re-exports from graph-sdk** — no extra publishing. Only a small set is genuinely irreducible.

Owner principle (verbatim intent): *the API builds **on** the engine and augments as needed; graph-sdk includes the engine; publish the residual only if graph-sdk is insufficient.* This ADR executes exactly that.

## Decision

### D1 — A 4-package published residual (the open-core boundary)
The product installs the engine from npm via **`@ailu/graph-sdk` + `@ailu/contracts` + `@ailu/napi` + `@ailu/config`** — and nothing else. graph-sdk is the door (it inlines the rest); `contracts` is the API↔Studio DTO boundary (intentionally separate); `napi` is the prebuilt native addon (external, zero-Rust-toolchain install); `config` is control-plane env parsing (kept external by design so the SDK never embeds the DB schema). `release.yml` is extended from the current set to publish `contracts` + `config` as well (graph-sdk + napi + cli already publish).

### D2 — graph-sdk is the door: additive re-exports absorb the inlined packages
graph-sdk gains **additive re-exports** so the product imports engine surface through the one door, not unpublished internals:
- `graph-core`: `validateGraph`, `GraphStateSchema`, `NodeType`, `NodeDefinition`, `EdgeDefinition`, `EdgeId`, `GraphId`, `GraphValidationError`, `GraphStatus` (alongside the existing `GraphDefinition`/`GraphState`/`RunId`/`NodeId`).
- `graph-runtime`: `GraphRuntime`, `InMemoryConditionRegistry`, `InMemoryEventBus`, `InMemoryNodeRegistry`, and the `Checkpointer` / `Checkpoint` / `CheckpointId` / `InterruptConfig` types.
- `agents-core`: `ReActAgent`, `AgentId`.
- `llm-gateway`: `LLMModel`, `LLMProviderAdapter`, `LLMRequest`.
- `search` + `memory-store`: added to the tsup workspace alias (zero `@ailu` deps → trivially inlinable), then re-export `SearchProvider`/`InMemorySearchProvider`/`SearchDocument`/`SearchHit`/`SearchResourceType`/`SearchQueryOptions`/`DEFAULT_SEARCH_LIMIT` and `BaseStore`.

All additive; identity is preserved because graph-sdk aliases to the same inlined source (so `implements` in `db-adapters` still type-checks).

### D3 — Governance/storage interfaces re-exported from graph-sdk *(owner-confirmed; mandatory-review item)*
graph-sdk re-exports the governed-interface **types + in-memory defaults** the control plane implements: `ApprovalEngine`, `ApprovalId`, `ApprovalRequest`, `RequestApprovalParams`, the approval error classes, `Ed25519Attestor`, `InMemoryApprovalEngine`, `canonicalJson`; `ArtifactStore`, `Artifact`, `ArtifactId`, `ArtifactVersion`. `db-adapters`' `PgApprovalEngine` / `PgArtifactStore` / `PgCheckpointer` then `implements` these via the door. **This widens graph-sdk's public governance/storage surface** (it currently declines to export them by policy) — the explicit mandatory-review change in this ADR. It is in-bundle (no new publish) and additive; the tsup external guard must still hold (no DB-schema / transitive-private-dep embedding — verified in build).

### D4 — YAML compilation routes through graph-sdk → napi (Rust) *(owner-confirmed)*
graph-sdk exposes **`compileGraphYaml(yaml)`** wrapping the existing napi `compile_graph_yaml_json` (Rust `ailu_graph_ailu::compile_graph_yaml`, already shipped at `crates/bindings/src/lib.rs`). `registry.service.ts` + Studio compile through the door; the TS `@ailu/graph-ailu` import and its `../../../../../engine/...` **deep relative source paths are deleted**. For `lang-ailu` (prompt/agent/chain YAML), its Rust compiler is exposed via napi the same way (or, until then, a thin publish). Net: converge YAML compilation on the Rust engine; no TS-compiler packages in the residual.

### D5 — config published; product re-points to the door
`@ailu/config` (small, clean, zero `@ailu` deps) is published as part of the residual. The product (`apps/api`, `apps/worker`, `apps/studio`, `packages/ui`, `packages/db-adapters`) re-points its `@ailu/{graph-core,graph-runtime,agents-core,llm-gateway,artifact-store,approval-engine,search,memory-store,graph-ailu,lang-ailu}` imports to **`@ailu/graph-sdk`**, and depends on `graph-sdk` + `contracts` + `napi` + `config` at `^1.3.0`. `db` / `db-adapters` / `ui` stay product-private; the API's product-specific extras (billing, auth, enforcement) remain product-side, building **on** the engine.

## Invariants
1. **One door.** The product reaches the engine only through `graph-sdk` (+ the 3 external residuals). No `../../../engine/` deep imports, no unpublished-package deps.
2. **Open-core boundary explicit.** The published set is exactly `{graph-sdk, contracts, napi, config}` (+ cli + wheels for their own consumers). Anything else the product needs is absorbed by the door or stays product-private.
3. **Type identity preserved.** Re-exports alias the same inlined source, so `db-adapters`' `implements ApprovalEngine/ArtifactStore/Checkpointer` keeps type-checking.
4. **No schema/secret embedding.** `config` + `db` stay external to graph-sdk (the tsup guard); the published SDK never embeds the DB schema or control-plane env.
5. **The engine stays the source.** The product builds on the published engine; it never forks or duplicates engine contracts (no hand-redefined interfaces).

## Build plan / touch-points
**Public engine (`ailu-engine`):**
- `packages/graph-sdk/src/index.ts` — the D2 + D3 additive re-exports; `tsup.config` workspace alias += `search`, `memory-store`.
- `packages/graph-sdk/src/…` — a `compileGraphYaml(yaml)` wrapper over the napi `compile_graph_yaml_json` (D4); expose `lang-ailu`'s Rust compiler via napi if it is not yet (`crates/bindings`).
- `.github/workflows/release.yml` — publish `contracts` + `config` alongside graph-sdk/napi/cli.
- A `.rust.test.ts` / typecheck asserting the new re-exports resolve and `implements` against them holds.
- Doc-site: note the published-surface (the 4-package residual) on the SDK-parity / install pages.

**Private product (`Ailu/product`):**
- Re-point `apps/api`, `apps/worker`, `apps/studio`, `packages/ui`, `packages/db-adapters` imports to `@ailu/graph-sdk`; delete the `../../../../../engine/...` deep imports in `registry.service.ts`.
- `package.json`s: `@ailu/{graph-sdk,contracts,napi,config}` at `^1.3.0`; drop the other `@ailu/*` workspace deps.
- Drop `engine/packages/*` + `engine/crates/bindings` from the workspace glob once nothing resolves to them (the stale subtree retires).

## Sequencing (unblocks phase 14)
1. **(public)** Land D2 graph-sdk re-exports + D4 `compileGraphYaml` + the LOW-risk inlines (search, memory-store) — additive, on `1.3.0`.
2. **(public, mandatory review)** Land D3 governance/storage re-exports — this ADR's sign-off gates it.
3. **(public)** Extend `release.yml` for `contracts` + `config`; cut the **`v1.3.0`** tag → publish `{graph-sdk, contracts, napi, config}` (+ cli/wheels).
4. **(private)** Re-point the product to `^1.3.0`; verify `apps/api` builds + tests against the published engine.
5. **Phase 14** (ADR 0036) builds against the published `1.3.0` door.

## Risks (+ mitigations)
1. **Public-surface creep** — re-exporting governance/storage widens what the OSS SDK guarantees. Mitigated by: it is the engine's already-public interface set (not internals), additive, and gated by this ADR's review. Re-evaluate if it pulls transitive private deps (verify the tsup external guard).
2. **Type-identity drift** — if a re-export resolved to a *different* declaration than the inlined one, `implements` breaks. Mitigated: graph-sdk aliases to source; a typecheck test asserts it.
3. **Bundle bloat / schema embedding** — accidental inlining of `config`/`db`. Mitigated by keeping them external (the existing tsup guard) + a bundle-content check.
4. **Publish-pipeline unproven for `contracts`/`config`** — `release.yml` has only published the bundle set. Validate against a pre-release tag (`v1.3.0-rc.1`) before the real tag.
5. **lang-ailu Rust gap** — if its Rust compiler is not yet napi-exposed, prompt/agent/chain YAML compilation needs the exposure done first (or a thin interim publish). Scope check before deleting the TS import.
6. **Two-repo change** — engine (public) + product (private) move together; land engine first + publish, then re-point the product (never the reverse, or the product can't install).

## Reserves / next
Retire the `engine/` subtree from the private workspace once the product is fully on the published door. A future ADR may publish more of the engine granularly if a third party needs it; for now the 4-package residual is the boundary.
