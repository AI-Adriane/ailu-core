# ADR 0042 — Subgraphs on the catalog path + concurrent subgraph fan-out

- Status: **Proposed** — implementation ADR for product ADR 0068 (child workflows), signed off
  (merged, revised) at github.com/AI-Adriane/ailu-core docs/adr/0068-child-workflows.md. Mirrors the
  ADR 0041 pattern: this ADR is the engine-side design; merge here = sign-off to implement.
- Date: 2026-07-21 (D1 shipped as-designed, PR #173; D2/D3 revised 2026-07-21 before any code — see
  Revision below)
- Deciders: Mathieu (owner)
- Relates to: product ADR 0068 (child workflows, the governance/control-plane design this
  implements the engine half of), ADR 0038 (replay-as-evidence), ADR 0027 phase 4b (`mapAgents`
  dynamic fan-out — extended here, not replaced)

## Revision (2026-07-21) — `execute_map_subgraph` is a `GraphRuntime` method, not an `agents-core` function

Before writing any D2/D3 code, a closer look found the original design (`map_subgraph_handler` in
`crates/agents-core/src/node.rs`, mirroring `map_node_handler`'s shape) **cannot work as specified**:

`map_node_handler` returns a plain `NodeHandler` closure that is SELF-CONTAINED — it captures
`Arc<ReActAgent>` and needs nothing else, so it can be built once at node-registry-construction time
(before the `GraphRuntime` exists) exactly like every other agent/tool handler. `execute_subgraph`,
by contrast, is a **private async method on `GraphRuntime` itself** (`&self`) — it recursively calls
`self.start_with_ctx(...)`/`self.resume_with_ctx(...)` against the SAME runtime instance's
checkpointer and subgraph registry. A `NodeHandler` closure has no way to obtain `&self` of the
runtime that will eventually call it — registries are built and handed INTO `GraphRuntime::new(...)`,
so at closure-construction time the runtime doesn't exist yet. This is exactly why the engine
special-cases `NodeType::Subgraph` INSIDE `execute_node` (`if node.node_type == NodeType::Subgraph {
return self.execute_subgraph(...) }`, checked with `&self` access) instead of routing it through the
generic `node_registry` the way `mapAgents`/agent/tool nodes are — confirmed by reading `execute_node`,
not assumed.

**Corrected D2**: `execute_map_subgraph` is a new PRIVATE method on `GraphRuntime`
(`crates/graph-runtime/src/runtime.rs`, alongside `execute_subgraph`), dispatched from the SAME
`execute_node` special-case, not a registered `NodeHandler`. This also changes the authoring surface:
since `graph-runtime`'s structural dispatch only ever reads `NodeDefinition`'s own core fields (never
`EngineSpec`-side-channel data the way `runtime-bridge` resolves `mapAgents`), the fan-out spec needs
to be a genuine new field on `NodeDefinition` itself (`graph-core`, wire-shared with TS) — analogous
to the `fan_out: Option<FanOut>` field already there — not a `node.metadata.mapSubgraphs` carrier
resolved one layer up in `runtime-bridge`/TS. This is a materially more central change than the
original D2 (touches the core wire schema, not just `EngineSpec`), flagged explicitly here rather
than glossed over.

D1 (already shipped, PR #173) is unaffected by this revision — it was pure TS plumbing exposing an
already-existing Rust capability, no design correction needed there.

## Context

Product ADR 0068 (revised 2026-07-21, after this repo was audited) found that child-graph execution
already exists end to end on the Rust engine, tested, but is unreachable from the catalog path the
product actually runs:

- `NodeType::Subgraph` + `NodeDefinition.subgraph_id` (`crates/graph-core/src/types.rs`) — already
  part of the core, wire-shared node schema.
- `execute_subgraph` (`crates/graph-runtime/src/runtime.rs:913`) — recursive start/resume sharing
  the parent runtime's checkpointer, suspend propagation to the parent (itself a checkpointed
  transition — sets `state.version += 1`, calls `self.suspend(...)`), failure propagation
  (`RuntimeError::SubgraphFailed`).
- `EngineSpec.subgraphs: Vec<GraphDefinition>` (`crates/runtime-bridge/src/spec.rs`) — the wire
  field the Rust bridge reads to resolve a `subgraph_id` reference. Round-trip tested.
- All of the above is exercised by a real `describeIfRust` test suite
  (`packages/graph-sdk/src/subgraph.test.ts`) — but only via the **builder** path
  (`GraphBuilder.subgraph()` → `CompiledGraph`, which threads `parts.subgraphs` into `EngineSpec`).

The **catalog** path — `runCatalogGraph` (`packages/graph-sdk/src/run-catalog-graph.ts`), what the
product's `RunsService` actually calls for every real run — hardcodes `subgraphs: []` at line 359.
`RunCatalogGraphOptions` has no `subgraphs` field to populate it with. This is the entire engine-
side gap for the SINGLE-child case (product ADR 0068 D2).

Product ADR 0068 D3 additionally wants a DYNAMIC fan-out of N independently-attested children (e.g.
"process each of these 40 invoices as its own governed sub-run") — the `mapAgents`-shaped UX, but
with subgraph children instead of agent children. This repo's `mapAgents` dispatch
(`crates/agents-core/src/node.rs:118`, `map_node_handler`) is hard-typed to `Arc<ReActAgent>`: it
calls `agent.run_scoped(item, ...)` per array item, `join_all`'d for concurrency, merged in input
order into the `join_at` channel. It has no path to invoke `execute_subgraph` instead of an agent —
confirmed by reading the function, not assumed. **This resolves product ADR 0068's flagged open
question**: D3 is NOT free: a genuinely new, parallel dispatch function is needed.

## Decision

### D1 — Expose `subgraphs` on `RunCatalogGraphOptions` (single-child case, product ADR 0068 D2)

Add `subgraphs?: GraphDefinition[]` to `RunCatalogGraphOptions` (and the matching `Pick<...>` on
`resumeCatalogGraph`'s options — `replayCatalogGraph` does not need it, since a subgraph node's
execution is itself journaled as part of the parent's replay just like any other node transition).
`run-catalog-graph.ts` threads it straight into `EngineSpec.subgraphs`, replacing the hardcoded
`[]` — the exact field `compiled-graph.ts:320` already populates for the builder path. No Rust
changes: `execute_subgraph`/`EngineSpec.subgraphs` already do the work; this is purely exposing an
existing capability through a currently-closed door.

A catalog graph authors a child call exactly like the builder path does: a node with
`type: "subgraph"`, `subgraphId` naming an entry in the `subgraphs` array. No new authoring surface
— `NodeType::Subgraph`/`subgraph_id` are already core `NodeDefinition` fields (unlike agent/
component nodes, not carrier-based metadata), so the catalog assembly (`run-catalog-graph.ts` on the
product side) needs no new node-metadata carrier, just to pass the `subgraphs` array through.

### D2 — `execute_map_subgraph`: a new `GraphRuntime` method, dispatched like `execute_subgraph`

A new field on `NodeDefinition` (`crates/graph-core/src/types.rs`), alongside the existing
`fan_out: Option<FanOut>`:

```rust
pub struct MapSubgraph {
    pub over_channel: String,
    pub join_at: String,
}
// on NodeDefinition:
pub map_subgraph: Option<MapSubgraph>,
```

`subgraph_id` is NOT duplicated here — a `map_subgraph`-bearing node is still `NodeType::Subgraph`
with its existing `subgraph_id`, just fanning out over an array instead of running once.

In `execute_node`, the existing check becomes:

```rust
if node.node_type == NodeType::Subgraph {
    if let Some(map_spec) = &node.map_subgraph {
        return self.execute_map_subgraph(&node, node_id, state, ctx, map_spec).await;
    }
    return self.execute_subgraph(&node, node_id, state, ctx).await;
}
```

`execute_map_subgraph` (new private method, `&self` access, alongside `execute_subgraph`): reads
`over_channel` as a JSON array, one child-run spawn per item — deterministic id
`{parent_run_id}:{node_id}:{index}` (extends `execute_subgraph`'s existing
`{parent_run_id}:{node_id}` convention with an index suffix), spawns run CONCURRENTLY via
`futures_util::future::join_all` over `Box::pin(self.start_with_ctx(...))`/`resume_with_ctx(...)`
calls — the SAME primitives `execute_subgraph` already uses per-child, just N of them at once, same
concurrency approach `map_node_handler` already uses for agent fan-out (so the pattern is still
"mirrored", just at the right layer). Writes each child's terminal output (or records its ongoing
suspension) into `join_at` as a JSON array, input-order preserved by `join_all`.

**Behavioral deviation from `execute_subgraph`, called out explicitly**: a per-item child FAILURE is
captured as `{ "error": ... }` at that array index — it does NOT fail the whole node (unlike the
single-child `execute_subgraph`, which propagates a child failure as the parent's own error). This
matches `map_node_handler`'s existing per-spawn error convention (one bad invoice shouldn't sink the
other 39) and is the more useful default for the N-child batch use case ADR 0068 named — a graph
author who wants "any failure fails everything" can still check the `join_at` array's error entries
via ordinary conditional routing.

### D3 — Concurrent suspension under fan-out: extend, don't reinvent

`execute_subgraph`'s existing suspend propagation (single-child case) sets the PARENT run suspended
when ONE child suspends. Under `execute_map_subgraph`'s N-concurrent-children case, if MULTIPLE
children suspend simultaneously (e.g. 3 of 40 invoices hit a human-gate), the parent suspends once,
carrying ALL outstanding children's suspended snapshots (extends the existing
`__subgraphStates`-keyed-by-child-run-id channel, which is already a map, not a single slot — no
schema change, just multiple entries populated instead of one). Resume re-attaches to every
outstanding child by iterating that map, exactly as the single-child path already does per key.

## Invariants (carried over from product ADR 0068, engine-enforced where noted)

1. **No new checkpoint semantics.** `execute_map_subgraph`'s node execution is itself one
   checkpointed transition on the parent, like any other node — concurrent children execute inside
   that one transition, not as separate checkpoint events on the parent's own stream.
2. **Approval gates evaluate fresh per child**, holds by construction — each concurrent
   `start_with_ctx`/`resume_with_ctx` call (D2) reuses the SAME `approvalEngine` the parent run was
   given, and each child's own gated tool call carries its own `node_id`-scoped
   `requestedBy`/subject. Confirmed by reading the existing single-child seam;
   `execute_map_subgraph` doesn't change this.
3. **Deterministic child ids.** `{parent_run_id}:{node_id}:{index}` — stable across a resume (the
   control plane can always re-derive which id belongs to which array item), never a randomly
   generated id that would break resumability.

## Consequences

- `packages/graph-sdk`: `RunCatalogGraphOptions.subgraphs` (new, optional), `run-catalog-graph.ts`
  wiring (D1) — small, mechanical. **Shipped, PR #173.**
- `crates/graph-core`: new `NodeDefinition.map_subgraph: Option<MapSubgraph>` field (D2) — a core
  wire-schema change, shared TS/Rust, more central than D1's `EngineSpec`-only addition. Reviewed
  explicitly as such (see Revision above).
- `crates/graph-runtime`: new private `execute_map_subgraph` method on `GraphRuntime` (D2/D3) —
  genuinely new code, but reuses `execute_subgraph`'s exact per-child primitives
  (`start_with_ctx`/`resume_with_ctx`) and `map_node_handler`'s exact concurrency approach
  (`join_all`, input-order merge) — no new concurrency or suspend/resume primitive invented, just
  composed at the right layer. Extends `__subgraphStates` to carry N entries instead of assuming one.
- `crates/graph-sdk` TS wire types: `MapSubgraph` on `NodeDefinition`, mirroring `FanOut`'s existing
  shape.
- Product-side control-plane work (giving each child a real `runsTable` row + attestation) is OUT
  OF SCOPE here — that is product ADR 0068's own remaining work, layered on top of this engine
  surface, not part of this ADR.

## Rejected

- **`map_subgraph_handler` as an `agents-core` function mirroring `map_node_handler`'s exact shape**
  (the ORIGINAL D2, before this Revision). Rejected once a closer look found it architecturally
  impossible — a `NodeHandler` closure has no way to obtain runtime (`&self`) access at
  construction time, which subgraph recursion structurally needs. See Revision above.
- **A single unified "map" node kind covering both agents and subgraphs at authoring time.**
  Rejected for the same reason product ADR 0068 rejected merging `mapAgents` and the new subgraph
  fan-out at the product layer: they're deliberately two choices (cheap/disposable vs.
  governed/independently-attested), and conflating them at the wire-schema level would obscure
  which one a graph author is actually getting.
- **Propagating a per-item child failure as the whole node's failure** (matching single-child
  `execute_subgraph`'s behavior exactly). Rejected in favor of `map_node_handler`'s existing
  per-spawn `{ "error": ... }` convention — more useful for the named batch use case, and a graph
  author who wants stricter all-or-nothing semantics can still build it via ordinary conditional
  routing over the `join_at` array's error entries.

## Build plan

1. D1 (small): `RunCatalogGraphOptions.subgraphs`, `run-catalog-graph.ts` wiring, a
   `describeIfRust` test proving a catalog-authored single-child subgraph resumes/attests correctly
   (mirrors `subgraph.test.ts`'s existing builder-path coverage). **Shipped, PR #173.**
2. D2/D3 (the real work): `NodeDefinition.map_subgraph` field (`graph-core`), `execute_map_subgraph`
   method (`graph-runtime`), TS `MapSubgraph` wire type + `GraphBuilder` authoring sugar (mirrors
   `.subgraph()`) + catalog-path `NodeDefinition` support. Tests: N-concurrent-children happy path,
   a subset suspending while others complete, a subset failing (captured per-index, others
   unaffected) without failing the whole node, resume re-attaching to all outstanding children after
   a process restart.
3. Release + product repoints (standard flow) → product ADR 0068's control-plane recursion
   (`persistCatalogOutcome` walking newly-terminal child ids) becomes buildable.
