# ADR 0001 — Keep the engine in TypeScript; do not rewrite in Rust

- Status: Superseded by [ADR 0002](0002-migrate-engine-to-rust.md) (2026-06-10)
- Date: 2026-06-10

> **Superseded.** The product owner elected to migrate the OSS engine to Rust despite
> the recommendation below. ADR 0002 records that decision and the staged plan. The
> robustness fixes proposed here (cycle-safe equality, checkpoint re-validation) were
> still shipped to the TS engine, which remains the working engine during migration.

## Context

Comparable agentic projects (ruflo, Cognitum's v0 Appliance) implement parts of
their stack in Rust, which raised the question: should Ailu's open-source engine
(`packages/*`) be rewritten in Rust to be "more robust"?

Relevant facts about Ailu today:

- The engine is ~10k LOC of strict TypeScript with green build/test/lint and a real
  SDK, runtime, agent layer, approval engine and DSL compilers.
- The control plane (`apps/api`, NestJS) and the Studio (`apps/studio`, Next.js) are
  TypeScript/React. A Rust engine would not change that — the commercial surface stays
  TS, so a rewrite buys a *polyglot* repo, not a Rust one.
- The workload is **I/O-bound**: an agent run is dominated by LLM/network latency and
  human-approval waits, not CPU. Determinism and resumability — Ailu's core contract
  — are correctness properties, not throughput properties.
- "Robustness" problems we actually observed were logic bugs reachable in any language:
  `JSON.stringify`-based equality (circular-ref crash, key-order false negatives) and
  un-validated persisted checkpoints — both fixable in TS in an afternoon.

## Decision

**Keep the engine in TypeScript.** Do not undertake a wholesale Rust rewrite.

Instead, pursue robustness where it pays off, in TS:

1. Cycle-safe `structuralEqual` replacing `JSON.stringify` comparison (done).
2. Zod re-validation of persisted checkpoints on load (`parseGraphState`, done).
3. Continue: schema-validate all data crossing the persistence/HTTP boundary;
   keep errors as typed discriminated results.

Reconsider Rust only as a **narrow, embedded core compiled to WASM** (via e.g.
`napi-rs`/`wasm-bindgen`) for a specific need where it demonstrably wins — most
plausibly a **deterministic serialization + cryptographic attestation** module for
tamper-evident audit trails (the differentiator borrowed from Cognitum), or an
edge/no-GPU execution target. That is additive and behind a stable TS interface; it
is not a rewrite.

## Consequences

- The substantial, tested TS engine is preserved; no multi-month rewrite, no
  ecosystem reset (Zod, Drizzle, Vitest, Nest, Next all stay).
- Single language across engine + control plane + Studio keeps contributor onboarding
  and the open-core story simple.
- We forgo native single-binary/edge deployment for now; if a sovereign/edge offering
  becomes a priority, the WASM-core path above is the entry point — and an attestation
  core is the first candidate, not the graph runtime.
- Robustness is addressed by validation-at-boundaries and property-focused tests, which
  is where the real defects were.
