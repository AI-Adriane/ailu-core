# ADR 0049 — The host keeps each checkpoint

- Status: **Proposed** — waits for the owner (public API, runtime invariant).
- Date: 2026-10-06
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0115 (catalog runs checkpoint every node durably) and its
  Revision 1. On 2026-10-06, asked « Point de reprise durable après chaque nœud (moteur +
  produit) : j'écris l'ADR maintenant ? », the owner answered « Oui, ADR moteur + produit ». This
  is the engine half.
- Relates to: [0044](./0044-cooperative-run-cancellation.md) (a run stops at a node boundary,
  after that node's checkpoint), [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first,
  both SDKs in the same release; the effect key of a host node).

## Context

The runtime checkpoints after every node completion and state mutation:
`GraphRuntime::persist_checkpoint` writes a `Checkpoint` before the next node runs. On the catalog
entry points — `runCatalogGraph` / `resumeCatalogGraph` (TypeScript), `run_catalog_graph` /
`resume_catalog_graph` (Python), and their C-API twins — the bridge builds a fresh runtime per call,
whose checkpointer is an `InMemoryCheckpointer`. Every node is checkpointed, but into memory that is
dropped when the call returns. The host only sees the call's outcome.

So a host that runs the engine for hours cannot keep a run durable between two calls. A host
process that dies at node 7 of 9 has nothing past the call's start. The legacy TypeScript
`GraphRuntime` takes a `Checkpointer` and writes every node. The native path, used by every product
run, does not.

Three more gaps follow from the same cause:

- The host cannot resume a run that was **running** when its process died. The checkpoint it would
  resume from never left the engine's memory.
- A `RunEvent` does not name the checkpoint its transition wrote, so a host cannot link its trace
  to the exact checkpoint.
- After a resume, the node in flight runs again. A host node already gets an effect key (ADR 0045
  D1) to act at most once. A host tool gets none.

## Decision

### D1 — The catalog entry points take a checkpointer, and the runtime awaits it

- `RunCatalogGraphOptions` and `ResumeCatalogGraphOptions` (TypeScript) gain `checkpointer?:
  Checkpointer`, the port the package already exports. Python gains `checkpointer=`, and the C-API
  gains `on_checkpoint` in a new callbacks version (`AiluCallbacksV3`).
- At every `persist_checkpoint` — node completion, state mutation, suspension, cancellation — the
  runtime hands the checkpoint to the host's `save` **and awaits it** before the next node runs.
  The seam is the same kind as `on_node`: an async callback the Rust side awaits (napi
  `ThreadsafeFunction::call_async`, a Python awaitable, a C completion callback).
- A `save` that fails stops the run with a typed error, `CheckpointSaveFailed`, carrying the
  checkpoint id. A run never continues past a checkpoint the host could not keep. Retries are the
  host's: it knows its store.
- Without a checkpointer, behaviour is unchanged: the in-memory checkpointer, the outcome only.

### D2 — Each lifecycle event names its checkpoint

`RunEvent` gains an additive `checkpointId`: the checkpoint written by the transition the event
reports. An event that writes none (a token delta) carries none.

### D3 — A running checkpoint resumes by re-running its node

`resumeCatalogGraph` accepts a state whose status is `running`, as left by a process that died
mid-run, and re-runs its current node. `resume_with_ctx` already does this for a node that is not
a gate. This ADR makes it a contract with one test per language: a run checkpointed after node 6,
resumed, runs node 7 again and then the rest.

### D4 — A host tool gets an effect key, like a host node

Each host-tool call carries `effectKey`: sha256 of `(runId, nodeId, state version at the node's
entry, tool name, canonical JSON of the call's arguments)`. A call repeated after a resume gets the
same key, so a tool that acts outside the engine can refuse the duplicate. A call with other
arguments gets another key, because it is another effect. The engine cannot make an effect
idempotent; it gives the host the key that makes it possible.

### D5 — One change, three languages, one release

D1–D4 ship together in Rust, TypeScript and Python (and the C-API) in **2.6.0**, after the 2.5.0
tag. The parity rule of ADR 0045 applies: no capability in one SDK only.

## Consequences

- A host can make every native run durable at every node. The product wires its Postgres
  checkpointer (product ADR 0115 D3), resumes a redelivered run from its last checkpoint (Revision 1
  D5), and fences the writes with its own lease (D6).
- One awaited host call per checkpoint: the node latency grows by the host's write time, often
  milliseconds next to a model call of seconds. The product measures it before making it its
  default (ADR 0115 D4).
- The public API grows additively. Existing callers that pass no checkpointer see no change.
- Replay is untouched: it reads the journal, not checkpoints.

## Not decided here

- Durability tiers (for example asynchronous saves for runs without side effects). The product's
  ADR 0115 asks the owner; if tiers come, they arrive as an option of D1, not as a new seam.
- How long a host keeps node checkpoints. That is the host's retention (product ADR 0115 D4).
