# ADR 0041 — Host tools on catalog runs, replayed and attested (agentic retrieval unlock)

- **Status:** Proposed (awaiting owner sign-off)
- **Date:** 2026-07-18
- **Relates to:** [0038 replay-as-evidence](./0038-replay-as-evidence.md) (the journal this extends), [0040 verify-replay control-plane](./0040-verify-replay-control-plane.md), [0037 product-engine-consumption](./0037-product-engine-consumption.md); product-side: ADR 0063 (Governed Agentic Mesh) workstream 1 / issue prxmat/ailu#431 (G4), GOV-1 version pinning (definition hash per run).

## Context

The product's 2026-07-17/18 architecture audits (issue prxmat/ailu#431) established three facts:

1. **The host-tool seam exists and works — but only on the builder path.** A Rust agent can call a
   JS tool mid-ReAct-loop through the napi `on_node` callback (`kind: "tool"` —
   `crates/runtime-bridge/src/lib.rs` `host_tool_handler`; `packages/graph-sdk/src/agent-node.ts`
   `RustToolBinding`). The in-process builder path wires it (`compiled-graph.ts`:
   `jsToolNames: new Set(toolFns.keys())`).
2. **The catalog path — everything the product runs — locks it shut.**
   `run-catalog-graph.ts` hard-codes `toolBindings: []` and `jsToolNames: new Set()`; an unknown
   `toolName` on a catalog agent becomes a no-op stub in the bridge. This is why the product's
   Governed Ask agent cannot *decide* to retrieve — the true blocker of agentic RAG (G4).
3. **Replay + attestation don't cover dynamic tool I/O or gate-less runs.** The ADR 0038 journal
   records LLM I/O + clock only (`ReplayJournalWire`); a dynamic host tool re-executing on replay
   would break determinism. And attestation records exist per APPROVAL only — a run with no gate
   produces zero signed proof.

Opening host tools to catalog runs WITHOUT extending the journal would silently downgrade
replay-as-proof — the one guarantee the platform sells. Hence one ADR for the three moves.

## Decision

### D1 — `RunCatalogGraphOptions.tools` (SDK, TS-only)

`runCatalogGraph` / `resumeCatalogGraph` / `replayCatalogGraph` accept an optional
`tools?: RustToolBinding[]` (name + `execute`). The catalog assembler threads them into
`RustRunnerParts` exactly as the builder path does (`toolFns` map + `jsToolNames`), so a catalog
agent whose `toolNames` includes a bound name gets a REAL tool instead of a no-op stub. Unbound
names keep today's stub behaviour (no breaking change; graphs stay data — the bindings are
call-site-supplied, never persisted).

### D2 — Host-tool results join the replay journal (Rust)

`ReplayJournalWire` gains a third stream: `toolResults: [{ name, inputHash, resultJson }]`, in
call order alongside `decisions` + `clock`.

- **Record mode:** every host-tool invocation's result is captured (canonical JSON; the input is
  hashed, not stored — inputs are reproduced deterministically by the replayed LLM decisions, the
  hash guards against divergence).
- **Replay mode:** host tools are NEVER re-executed. The bridge serves `toolResults[i]` in order,
  verifying the recorded `inputHash` against the replayed call's input; a mismatch fails the
  replay verdict (faithfulness signal, same doctrine as approval-sequence comparison in 0040).
- A journal without `toolResults` (pre-0041) replays exactly as today — the field is additive.

### D3 — Terminal attestation for every run (control-plane, uses GOV-1)

The control plane appends a terminal attestation record to the chain for EVERY catalog run —
gated or not: `{ runId, definitionHash (GOV-1 pin), entryStateHash, terminalStateHash,
journalHash, status }`, Ed25519-signed by the existing instance key. Gate approvals keep their
per-approval records; this adds the run-level "what ran, from what, to what" proof so *"every run
attested"* stops being conditional on a gate firing. (Engine change: none — `Ed25519Attestor` and
the chain verifier already accept heterogeneous records; the record shape lands in the product's
attestation writer.)

## Consequences

- G4 becomes a small product change: register a `search_brain` binding backed by the control
  plane's governed recall, put `toolNames: ["search_brain"]` on the Governed Ask answer node —
  the agent decides IF and WHEN to retrieve, and the run stays replayable + attested.
- G5 (multi-source) rides the same seam: `web_search`/`http_fetch`/KB bindings become agent
  choices under the same journal.
- Journal size grows with tool traffic (bounded: results are the same data the agent already put
  in its transcript); acceptable, mirrors the 0038 "full LLM I/O" decision.
- Replay of a pre-0041 run is unchanged; verify-replay gains one new failure class
  (`tool_input_mismatch`).

## Rejected alternatives

- **Advisory, non-replayable mode for tool-using runs** — rejected: creates a two-tier trust
  story and quietly breaks the product's core claim.
- **Rust-native brain tool (HTTP from the engine to the control plane)** — rejected: inverts the
  dependency rule (engine must never know the control plane) and duplicates the host-tool seam
  that already exists.
- **Keep retrieval as seeded channels only** (status quo) — rejected by the owner's agentic-RAG
  direction (issue #431); static seeding stays as the fallback when no binding is supplied.

## Phasing

1. **E1 (TS, this repo):** D1 — options threading + tests (a catalog agent calls a bound tool;
   unbound stays stub). No Rust change; ships in a minor release.
2. **E2 (Rust, this repo):** D2 — journal stream + record/replay/verify + tests. Ships with E1 or
   right after (E1 alone is gated to non-record runs by the product until E2 lands).
3. **E3 (product):** `search_brain` binding + G4 wiring + `tool_input_mismatch` surfaced in
   verify-replay.
4. **E4 (product):** D3 — terminal attestation writer using the GOV-1 pin.
