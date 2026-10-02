# ADR 0045 — Host nodes in the Rust engine, journaled; and every SDK binds the same engine

- Status: **Accepted** — 2026-10-02, by the owner (Mathieu: « oui, débloquer déjà la 0096 »). M1
  scope adjusted at acceptance — see Phasing.
- Date: 2026-10-02
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0096 (an action from Work, behind a gate: "the agent proposes,
  a human signs, the control plane executes"), whose tool step cannot run on the catalog path
  today; and by the owner's direction of 2026-10-02: **the engine is Rust; TypeScript and Python
  are language layers over it, and both get every capability.**
- Relates to: [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (host tools on
  catalog runs, their journal), [0038](./0038-replay-as-evidence.md) (replay as evidence),
  [0044](./0044-cooperative-run-cancellation.md) (cancellation at a node boundary).

## Context

Read from the source on 2026-10-02 (release 2.1.0).

### The step ADR 0096 needs does not run on the catalog path

- The Rust bridge **already** runs a node in the host: an id listed in `EngineSpec.jsNodeIds`
  gets `host_node_handler` (`crates/runtime-bridge/src/lib.rs`), which calls the host's `on_node`
  callback (`kind: "node"`) and applies the returned channel update.
- The TypeScript catalog seam **disables it**: `assembleParts`
  (`packages/graph-sdk/src/run-catalog-graph.ts`) gives every node without a component or agent
  carrier the function `async () => ({})`, and `RunCatalogGraphOptions` offers host **tools**
  (ADR 0041 D1) but no host **node**. The product runs only catalog graphs: it cannot execute code
  of its own as a step.
- A host node is **not journaled**. Host tools are (ADR 0041 D2, `tool_journal.rs`): a replay
  serves their recorded results and never calls them. A host node is called again on replay —
  harmless for a pure step, wrong for a step that writes to the outside world (a message, an
  e-mail draft): a replay would send it again.

### The three bindings do not expose the same engine

| Binding                     | Language                                                                    | What it exposes                                                                                                                                             |
| --------------------------- | --------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| N-API (`crates/bindings`)   | TypeScript                                                                  | the callback-capable runner (`engine_run`, `engine_resume`, `engine_approve_and_resume`, `engine_signal`, `engine_replay`), `llm_complete`, validation, DSL |
| C ABI (`crates/c-api`)      | Go, Ruby, PHP, Lua, PowerShell, C/C++, Zig, Swift, Obj-C, JVM, .NET, Elixir | the same callback-capable runner (no `is_cancelled` callback), validation, DSL, catalogs, one-shot component/prebuilt runs                                  |
| PyO3 (`crates/py-bindings`) | Python                                                                      | **nine callback-free functions only**: version, validate, compile DSL, model policy (2), catalogs (2), run one component, run one prebuilt agent            |

**Python cannot run a graph.** The C ABI exposes the runner and the Ruby, Java, C#, Swift, PHP,
Lua, PowerShell, C++ and Zig SDKs wrap it (Go and Elixir do not yet); the Python SDK does not.

### Engine logic lives in TypeScript, so the other SDKs do not have it

Beyond the deprecated fallbacks (below), the TypeScript SDK carries logic the Rust engine does not:

| TypeScript-only logic                                                                                          | Where                                                                                                                   | Consequence                                                                                                                                                              |
| -------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Filing a suspended run's approval requests (tools, gates, direct child runs) and checking them before a resume | `fileApprovalRequests`, `ensureApprovalsGranted` (`run-catalog-graph.ts`)                                               | a run started through the C ABI (or Python, once it can run) files no approval and checks none unless its host re-implements this: **governance exists in one SDK only** |
| Turning carriers into an `EngineSpec`                                                                          | `assembleParts`, `carrierToAgentConfig`                                                                                 | every other SDK would have to re-implement it                                                                                                                            |
| Ed25519 attestation and chain verification, canonical JSON                                                     | `packages/approval-engine/src/attestation.ts` (used by the product) **and** `crates/approval-engine/src/attestation.rs` | two implementations of one proof format — a divergence would make a proof valid in one language and invalid in the other                                                 |
| Replay verification, run explanation, offline certificate verifier                                             | `verifyReplayDecisions`, `explainRun`, `@ailu-ai/verify`                                                                | not available outside TypeScript                                                                                                                                         |
| OTLP trace export                                                                                              | `observability.ts`                                                                                                      | TypeScript only                                                                                                                                                          |

The packages marked `[DEPRECATED — … fallback when the @ailu-ai/napi native addon is absent]` —
`graph-runtime`, `agents-core`, `llm-gateway`, `runnable`, `callbacks`, `observability`,
`artifact-store`, `memory-store`, `rag-pipeline`, `approval-engine`, `lang-ailu`, `graph-ailu` —
are TypeScript copies of crates. ADR 0016 retires them; they stay while a native binary is missing
for a supported platform (Intel macOS, ailu-core#239).

## Decision

### D1 — A host node is a Rust capability: bound by id, journaled, never called by a replay

1. **`EngineSpec.hostNodeIds`** names the nodes whose step is the host's (`jsNodeIds` stays
   accepted as an alias). Unbound nodes keep today's no-op.
2. **An effect key per execution.** The `on_node` payload of a host node carries `effectKey`:
   sha256 of `(runId, nodeId, state version at the node's entry)`. A step retried from the same
   checkpoint (the worker is at-least-once) gets the same key; the host uses it to execute an
   external effect at most once. The engine cannot make an external effect idempotent — it gives
   the host the key that makes it possible.
3. **Journaled like a host tool.** In record mode, every host-node result is appended to the replay
   journal: `nodeResults: [{ nodeId, inputHash, update | error }]` (input = the node's entry
   channels, hashed, never stored), next to `decisions`, `clock` and `toolResults`.
4. **A replay never calls a host node.** It serves the recorded update for `(nodeId, inputHash)`;
   no match is a divergence (`node_input_mismatch`) and fails the replay — never a live call. A
   journal recorded before this ADR has no `nodeResults`: its replay keeps today's behaviour (a
   builder graph's pure JavaScript steps are re-derived), so old evidence stays verifiable.

### D2 — Every SDK binds the same engine, in the same release

1. **A capability is defined in Rust first** (spec, bridge, journal), then exposed by the
   TypeScript and Python SDKs **in the same release**. A release that adds a seam to one SDK only
   does not ship (RELEASING.md gains the check).
2. **Python gets the callback-capable runner** (PyO3): `run`, `resume`, `approve_and_resume`,
   `signal`, `replay`, taking the same `EngineSpec` and Python callables — host nodes and host
   tools (`on_node`), named conditions (`on_condition`), lifecycle events (`on_event`),
   cancellation (`is_cancelled`) — and a `run_catalog_graph(definition, *, nodes, tools,
initial_data, provider_keys, …)` that mirrors the TypeScript one.
3. **TypeScript**: `runCatalogGraph` / `resumeCatalogGraph` accept `nodes` (`{ id, execute(state)
→ channel update }`); `replayCatalogGraph` does not — and even if a host passed one, D1.4
   serves the journal.
4. **The C ABI** gains the `is_cancelled` callback (ADR 0044) its other SDKs miss.

### D3 — The engine logic that lives in TypeScript moves to Rust

In this order, each in its own release, exposed to both SDKs:

1. **Approvals on the catalog path**: which requests a suspended run files (tools, gates, direct
   child runs) and whether a resume is granted are decided by the engine; the host only stores
   them (an approval-store callback). Every SDK then governs the same way.
2. **Carriers to `EngineSpec`**: the engine reads a catalog `GraphDefinition` and its bindings
   itself (`spec_from_catalog`), instead of each SDK assembling the spec.
3. **One attestation implementation**: Ed25519 signing, verification, chain walking and canonical
   JSON in Rust only; the TypeScript `Ed25519Attestor` and `@ailu-ai/verify` wrap it.
4. **Replay verification and run explanation** in Rust.
5. **OTLP export** in the Rust observability crate (lowest priority: an exporter can stay a host
   concern).

### D4 — The deprecated TypeScript fallbacks go when every platform has a native binary

ADR 0016 is carried out: once a native binary is published for every supported platform, the
deprecated packages are removed and the SDK requires the engine — no second, divergent
implementation of the runtime.

## Phasing

| Step   | Content                                                                                                                                                                                | Release                           |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| **M1** | D1 (host nodes in Rust, effect key, journal, replay) + D2.2 (Python runner over an `EngineSpec`: host nodes, host tools, conditions, events, cancellation) + D2.3 (TypeScript `nodes`) | 2.2.0 — unblocks product ADR 0096 |
| M2     | D3.1–D3.2 (approvals and spec assembly in Rust) + Python `run_catalog_graph` + D2.4 (C ABI cancellation)                                                                               | 2.3.0                             |
| M3     | D3.3–D3.4 (one attestation; verify-replay, explain)                                                                                                                                    | 2.4.0                             |
| M4     | Python at the TypeScript level for the rest: graph builder, token streaming, `llm_complete`, durable helpers (`sleepUntil`, `waitForSignal`), embeddings and vector store              | 2.5.0                             |
| M5     | D4 — retire the deprecated fallbacks                                                                                                                                                   | 3.0.0 (breaking)                  |

Two items of D2 move from M1 to M2, decided at acceptance:

- **Python `run_catalog_graph`** needs a catalog definition turned into an `EngineSpec`. Today
  only TypeScript does that (`assembleParts`); writing it again in Python would add the very
  TypeScript-only logic D3 removes. It ships with D3.2 (`spec_from_catalog` in Rust). In M1,
  Python runs a graph from an `EngineSpec` — the same runner the TypeScript catalog path drives.
- **C ABI cancellation** adds a field to `AiluCallbacks`, a struct every C-ABI SDK lays out
  itself: appending it in place would make an SDK built against 2.1 pass a struct one field
  short. It needs a versioned entry point, designed with M2.

## Consequences

- Product ADR 0096 builds its `action-approval` graph on M1: a gate, then a host node keyed by the
  proposal; the at-most-once claim uses the effect key; a replay serves the recorded receipt and
  sends nothing.
- A journal grows by one entry per host-node execution (bounded by the graph).
- `node_input_mismatch` joins `tool_input_mismatch` as a verify-replay failure class.
- Python reaches graph execution in M1; full parity with TypeScript is M4, not M1 — said in the
  Python README until then.

## Rejected alternatives

- **A TypeScript-only `nodes` option** (the shape of ADR 0041 D1): it would ship a capability to
  one SDK, leave the replay re-calling effectful steps, and widen the TypeScript-only surface the
  owner wants to shrink.
- **An agent that only triggers a tool executing the signed parameters** (no engine change): puts
  a model call — its cost, its outages, a model that does not call — on the path of a signed
  action.
- **Executing the action outside the run**: the run would not carry or attest the receipt.
