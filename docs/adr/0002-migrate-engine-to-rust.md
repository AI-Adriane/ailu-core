# ADR 0002 — Migrate the open-source engine to Rust (staged)

- Status: Accepted
- Date: 2026-06-10
- Supersedes: [ADR 0001](0001-keep-typescript-engine.md)

## Context

ADR 0001 recommended staying in TypeScript. The product owner decided to migrate the
open-source engine (`packages/*`) to Rust, prioritising memory safety, a single
native binary / edge-deployable runtime, and alignment with the sovereign-appliance
direction (cf. Cognitum's Rust + MCP edge core). This ADR records that decision and,
more importantly, **how** to do it without throwing away a working, tested system.

## Decision

Migrate to Rust **incrementally, bottom-up, crate by crate**, with the TS engine
remaining the production engine until each Rust crate reaches parity and is adopted.

Boundaries:

- **In scope (OSS):** the engine packages — `graph-core`, `graph-runtime`,
  `agents-core`, `llm-gateway`, `approval-engine`, `artifact-store`, `memory-store`,
  `callbacks`, DSL compilers, CLI.
- **Out of scope:** the control plane (`apps/api`, NestJS) and Studio (`apps/studio`,
  Next.js) stay TypeScript. The Rust engine is consumed from Node via **napi-rs**
  native modules and/or a thin service boundary; the SDK keeps a TS facade.

### Order of migration

1. `ailu-graph-core` — pure model + validation. **(foundation written)**
2. `ailu-graph-runtime` — the executor: node/condition registries, checkpointer
   trait, event bus, the run loop, suspend/resume, fan-out, cycles. The crux.
3. `ailu-approval-engine` + attestation (Ed25519 already designed in TS; Rust is a
   natural home for the crypto/deterministic-serialization core).
4. `ailu-agents-core`, `ailu-llm-gateway` (provider adapters behind a trait).
5. Stores, callbacks, DSL, CLI.
6. **Node bindings** (napi-rs) so `apps/*` and the SDK call the Rust engine; flip
   consumers one at a time; delete the TS package once its Rust crate is adopted.

### Non-negotiables during migration

- **Wire compatibility:** Rust types (de)serialize to the exact camelCase JSON the TS
  model uses, so a checkpoint or graph definition is portable across both engines.
- **Behavioural parity via tests:** each crate ports the corresponding Vitest suite to
  `cargo test`; determinism, checkpoint-after-every-node, and clean human-gate
  suspend/resume must hold identically.
- **No flag day:** `main` always builds and passes. The TS engine is not deleted until
  its Rust replacement is wired and green.

## Consequences

- **Cost:** a multi-phase effort; `graph-runtime` (resumability, time-travel,
  fan-out, cycles, interrupts) is the hard part and the real test of the decision.
- **Polyglot repo:** Rust engine + TS control plane/Studio. Contributors need both
  toolchains; CI gains a `cargo` lane.
- **Toolchain:** Rust was not installed in the environment where the foundation was
  written, so `crates/graph-core` is authored but **unverified** — first action is
  `cd crates && cargo test`.
- **Payoff:** native single-binary/edge target, memory safety, and a clean home for
  the attestation/deterministic-serialization core. If the cost of phase 2 proves
  unjustified against these benefits, ADR 0001's reasoning still stands and we can
  stop with a hybrid (Rust core for crypto/edge, TS for orchestration).
