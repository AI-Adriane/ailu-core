# ADR 0045 — Host nodes in the Rust engine, journaled; and every SDK binds the same engine

- Status: **Accepted** — 2026-10-02, by the owner (Mathieu: « oui, débloquer déjà la 0096 »). M1
  scope adjusted at acceptance — see Phasing. **Revision 1 (D3.1): Accepted** — 2026-10-10, by
  the owner, with the architect's recommendations — see below.
- Date: 2026-10-02
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0096 (an action from Work, behind a gate: "the agent proposes,
  a human signs, the control plane executes"), whose tool step cannot run on the catalog path
  today; and by the owner's direction of 2026-10-02: **the engine is Rust; TypeScript and Python
  are language layers over it, and both get every capability.**
- Relates to: [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (host tools on
  catalog runs, their journal), [0038](./0038-replay-as-evidence.md) (replay as evidence),
  [0044](./0044-cooperative-run-cancellation.md) (cancellation at a node boundary).

## Revision 1 (2026-10-10) — D3.1 files a human gate at any subgraph depth and in every `mapSubgraph` item

- Status: **Accepted (Mathieu, 2026-10-10)**. The architect's review validated it with
  reservations, and the owner accepted the whole plan with all of that review's recommendations.
- Decided by the owner on 2026-10-10: such gates are **filed correctly**. Refusing the graphs is
  rejected (see Alternatives).
- Relates to: [0042](./0042-subgraphs-on-the-catalog-path.md) (subgraphs and `mapSubgraph` on the
  catalog path), [0043](./0043-recursive-replay-for-subgraphs.md) (the child run ids
  `{run_id}:{node_id}[:{index}]`), [0046](./0046-an-approval-above-a-threshold.md) (a wait is told
  apart by the call it holds), and product ADR 0068 (child workflows; Revisions 5 and 9 record
  the gap: no run gate at depth ≥ 2).

### The hole

D3.1 shipped with a scope stated in the module doc (`crates/runtime-bridge/src/catalog_approvals.rs:22-23`):

> a nested subgraph inside a child, and a `mapSubgraph` fan-out's children, are not walked.

`requests_of` (`catalog_approvals.rs:284-348`) reads two things only:

- the run's own state: its agent tool requests and its gate (`:292-294`);
- the snapshot of a **direct** `subgraph` child (`:296-346`). A `mapSubgraph` node is skipped
  (`:299-303`), and the walk does not descend into a child's own `__subgraphStates`.

The runtime suspends a parent at its subgraph node whenever its child suspends
(`crates/graph-runtime/src/runtime.rs:1275-1282`), at any depth: each level keeps its child's
snapshot in its **own** `__subgraphStates`. The map case works the same way, with one snapshot per
waiting item, keyed `<run>:<node>:<index>` (`runtime.rs:1378-1380`, `:1428-1438`, `:1482-1488`).
A resume of the parent re-enters the node, and every waiting child resumes. A child waiting at a
human gate **advances** past it (`runtime.rs:652-659`, `:1243-1244`, `:1408-1411`).

So at depth ≥ 2, or inside a `mapSubgraph` item:

- `filing_plan` files **nothing**;
- `resume_problems` sees no wait, because there are no stashed ids and `requests_of` is empty
  (`catalog_approvals.rs:447-458`);
- the first resume (a redelivered job, a « resume » click) **passes the gate with no human
  decision**.

Two red tests on the catalog path show this (`GovernedHost` in `crates/runtime-bridge/src/lib.rs`,
the harness of #331/#332, with a fresh runtime per call as the bindings build it). The repository
allows no skipped or ignored test, so they are not committed red. Each one lands, green, with the
PR of the plan that fixes it.

| Test                                                                      | Observed today                                                               |
| ------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `a_human_gate_two_subgraph_levels_down_is_filed_and_waits_for_a_decision` | nothing filed; the resume runs: `Completed`, the step past the gate ran once |
| `a_human_gate_inside_a_map_subgraph_is_filed_once_per_item` (2 items)     | nothing filed; the resume runs: `Completed`, both items acted                |

A third red test found a **runtime** defect that this revision must fix too, because it breaks
"one decision per item":

| Test                                                                     | Observed today                                                                                                                       |
| ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| `a_map_subgraph_item_that_completed_is_not_run_again_by_the_next_resume` | item A completes while item B waits at a second gate; the next resume starts A **from scratch**, and A waits at its first gate again |

The cause is in `execute_map_subgraph`:

- a completed item's snapshot is dropped (`runtime.rs:1452-1458`);
- on the next entry, every item without a snapshot is started again (`:1385-1421`). The comment
  at `:1483-1487` assumes this is "cheap, deterministic".

On a fresh runtime per call (napi, PyO3, C ABI), an item restarts from its entry. Repeated
resumes alternate between items until both happen to finish in the same call. A probe of the
two-item graph took four resumes after the first, with A's step running three times and B's
twice. In-process, the completed child's checkpoint is still there, and `resume_with_ctx` reruns
its last node (`runtime.rs:669-671`).

Once gates are filed, every restart would file the item's gate **again**, which means asking a
person to decide an item that was already decided. Side effects would also be repeated.

### Decision

Accepted by the owner on 2026-10-10. The architect validated the revision with reservations, and
Mathieu accepted the plan together with every recommendation of that review; those
recommendations are folded in below (N1–N8 name the review's points).

**R1 — One walk, any depth.** `requests_of` becomes a depth-first walk over the kept state. A
state's waits are:

- its own agent tool requests and its own human gate, as today;
- for each `subgraph` node of its graph, in declaration order, the waits of each child snapshot
  that is suspended, found in **that state's** `__subgraphStates`:
  - a direct child (no `mapSubgraph`): the run id recorded in `__subgraphRuns[node]`, otherwise
    `<run>:<node>` (`child_run_id`, `:275-281`), as today;
  - a `mapSubgraph` node: the walk reads the **keys** of `__subgraphStates` that are
    `<run>:<node>:` followed by a decimal index (N1). Its cost follows what waits, not the
    fan-out width. Items are walked in **ascending numeric index**: index 10 comes after index 2.
  - An index that is not below `len(overChannel)` of the suspended state means the list changed
    while the node waited (an `update_state`). The plan then carries a `refusal` (R5) instead of
    guessing which item is which.

  The walk recurses into each child with that child's graph (the last definition of that id wins,
  as at `:311-317`).

**R2 — The identity of a wait is the runtime's own run id.** A wait is filed under the run that
waits, at any depth:

- `runId` = the waiting child's run id, for example `run:outer:inner` or `run:each:1`, or
  `run:each:1:review` for a subgraph inside an item;
- `nodeId` = `requestedBy` = `<that run id>:<node>`;
- subject `gate:<that run id>:<node>`.

This is the convention a direct child already uses (`gate:run-1:sub:c_review`, test at `:844-866`),
generalized, so depth 1 is unchanged byte for byte and no golden case moves. The item index is in
the run id. The engine shows the index only; the product shows the signer the item, read from the
run's state with personal data masked, and copies no item data into the approval store.

**R3 — Every wait is filed together, and decided one by one.**

- _Filing._ A plan lists every wait of the state in walk order, so two items waiting at once give
  two requests in the same plan. The host files them all and stashes **all** their ids in the
  top-level state's `__approvalIds`, the only state it keeps. This is the mechanism a direct child
  uses today: the ids are never written into a child snapshot.
- _One decision per item._ Each request belongs to exactly one item's run, so approving one
  decides that item and no other.
- _Resume._ One host resume re-enters the node, and every waiting item advances
  (`runtime.rs:1398-1422`). So a resume is refused while **any** stashed request is pending,
  unknown, approved by its own requester, or a rejected gate (`resume_problems`, `:443-513`,
  unchanged). In the red test, approving item 0 alone still leaves the resume refused ("request
  id-1 … is still pending").
- _A rejected item ends the run_, as a rejected gate does today (product ADR 0068 Revision 10,
  ADR 0053 D4). An `{ "error": "rejected" }` join slot that lets the siblings go on is left to a
  later revision, if a product case asks for it.
- _Next wait._ After a resume, R1 of ADR 0046 and the #332 rule apply unchanged to the deep key:
  - `suspension_key` (`:356-378`) and `waits_at_a_gate` (`:389-394`) both go through
    `requests_of`, so they see deep waits;
  - an item that moved on to its next gate is a new wait: the stash is cleared and the gates now
    waited on are filed.
- _Idempotent filing_ (from PR 3, which brings N requests per wait). Each `ApprovalToFile` gains
  an additive `idempotencyKey`: the sha256 of `[runId, requestedBy, subject, version of the
suspended state]`. A host passes it to `ApprovalEngine.request()`. The product deduplicates on
  `(tenant_id, idempotency_key)`, which is a schema change reviewed on its side. A job redelivered
  after a crash then finds the requests it filed instead of filing them again.

**R4 — A completed item keeps its result (runtime, `execute_map_subgraph`).**

- When an item completes, or fails into its `{ "error": … }` slot, its **join element** is kept
  under a new engine-owned channel, `__mapResults`:
  `{ "<item run id>": { "itemHash": "<hex>", "result": <join element> } }`. The channel is added
  to `ENGINE_OWNED_CHANNELS` (`runtime.rs:68-74`), so neither a run's input nor a node's update
  can forge a finished item and skip its gate.
- **One mutation, one checkpoint (N2).** The kept elements are written in the **same** mutation as
  the node's suspension (or cancellation), before its version bump and `suspend()`
  (`runtime.rs:1460-1488`). The node does not complete, so this is not "the checkpoint after the
  node". The node's entries are dropped when it completes. The whole channel is dropped when the
  run fails, so a run never ends holding item results.
- **Hidden from views (N3).** A single `handler_view` in the runtime removes the hidden
  engine-owned channels from the state every node handler receives, at its three call sites
  (`runtime.rs:918`, `:1080`, `:1121`). An agent's seed serializes its whole state
  (`agents-core/src/react.rs:309-322`), and through this view it never sees item results. A map
  child never inherits the channel: without an input mapping, it would otherwise receive every
  parent channel (`runtime.rs:1377`). `__mapResults` is never put in an event. `__agentResume`
  (ADR 0053) joins the same list when it lands. There is one mechanism for the sensitive
  engine-owned channels, applied by the engine.
- **The same item only (N4).** `itemHash` is the sha256 of the item's canonical JSON (keys
  sorted). On re-entry, a kept result is reused only when the item at its index hashes the same.
  Otherwise, or for a kept entry whose index is out of range, the node fails (`Permanent`,
  explicit reason): the list changed while the node waited.
- On re-entry, an item with a kept element is **not run again**: it is neither restarted nor
  resumed, and it emits no event, because nothing in it changes state.
- When the node completes, the join is built in index order from the kept elements and the newly
  finished ones.
- A cancelled item keeps its snapshot, as today.
- **Replay (N5).** What R4 does depends only on the **presence** of `__mapResults` in the state a
  segment starts from:
  - a segment that starts without it (any segment recorded before R4) re-runs its items exactly
    as it did, so its journal still matches;
  - a replay runs one segment, from one checkpoint to the next suspension or the end
    (`replay_from`, `runtime.rs:769-791`), so no journal mark is needed.

  A test replays a journal recorded before R4.

The checkpoint after the node and the single `RunSuspended` / `RunResumed` / `NodeCompleted` per
transition are unchanged.

**R5 — Bounds (rule 120 style), fail closed.**

- A plan files at most `MAX_FILED_REQUESTS` = **256** requests, counted over the whole walk.
- The walk descends at most `MAX_WAIT_DEPTH` = **16** levels.
- Beyond either bound, or on an out-of-range index (R1):
  - the plan files nothing and carries a `refusal`, a new optional field, additive in all three
    bindings;
  - `resume_problems` returns the same text, for example "the run waits on 1000 approvals; at most
    256 are filed for one wait";
  - the host **fails the run** with that reason, instead of keeping a run nobody can decide.
- **Fail early, before any effect (N6).** At the entry of a `mapSubgraph` node whose child graph,
  at any depth, holds a human gate or an agent with an approval-gated tool, `len(items) >
MAX_FILED_REQUESTS` fails the node before any item runs (`Permanent`, explicit reason). The
  plan's bound stays as the second net.
- 256 is the engine's ceiling. A product may set a lower one in its policy.

**R6 — A tool grant cannot reach a child run yet: refused explicitly (N7).**

- Nothing writes `__approvedTools` into a child snapshot: the bridge writes the top-level state
  only (`lib.rs:512-518`), and a child resumes from its snapshot.
- So an approved tool request of a child agent loops:
  1. the agent starts again and asks for the same call;
  2. the wait key is the same (#331) and the stored request is approved, so the next resume
     passes;
  3. the agent asks again.

  Each turn calls the model again and costs, and a signer who said yes sees nothing happen. This
  exists today at depth 1, and R1 would extend it.

- **Now** (its own small PR, before PR 2):
  - `filing_plan` files no tool request of a child run, and carries the `refusal` « a tool
    approval in a child run cannot be granted yet (ADR 0045 rev. 1, R6) »;
  - `resume_problems` returns it first;
  - the SDKs raise it as a terminal error, so the host fails the run with the reason.
- **Then**, a short separate revision routes the grant: `ApprovedTool.runId`, and the bridge
  writes the validated grant (no self-approval, as today) into
  `__subgraphStates[<child run>].channels.__approvedTools`. Until then, an agent with an
  approval-gated tool inside a child workflow is refused, not looped.

### What does not change

- **Determinism.** `filing_plan` stays a pure function of `(graph, subgraphs, state,
previousState)`, with a fixed walk order. R4 makes the join a function of the recorded per-item
  results instead of a rerun. Every engine-owned write goes through a checkpoint.
- **Events and suspension.** R1–R3, R5 and R6 are host-side decisions and emit nothing. A
  suspended run is still the clean `run_suspended` at its subgraph node, with one per child run,
  as today. R4 removes the events a rerun produced, which belonged to no real transition.
- **Replay and verify.** A replay files nothing (`packages/graph-sdk/src/run-catalog-graph.ts:657`),
  and `pendingApprovals` lists top-level tool requests only (`lib.rs:2143-2164`). A gate decision
  at depth is attested under its child's run id, as at depth 1. R4's replay rule is N5 above.
- **TypeScript fallback.** There is none on the catalog path: `runCatalogGraph` /
  `resumeCatalogGraph` require the native engine (`run-catalog-graph.ts:487-488`, `:570-572`;
  `compiled-graph.ts:115-140`). The deprecated `@ailu-ai/graph-runtime` copy files no approvals.
  TypeScript and Python get the fix through the same native functions:
  - TypeScript: `fileApprovalRequests` / `ensureApprovalsGranted`, `run-catalog-graph.ts:730-800`;
  - Python: `_file_approvals` / `_ensure_approvals_granted`, `python/ailu/__init__.py:999-1071`.

  Parity is proven by the shared golden fixture (`crates/runtime-bridge/tests/fixtures/catalog_approvals_golden.json`,
  read by the Rust, TypeScript and Python tests), which gains nested and map cases.

### Old checkpoints

- **A state that waits at depth ≥ 2, or in a map item, kept before the fix.** It has no stashed
  ids. After the upgrade, `resume_problems` sees its waits and refuses with "the run waits on an
  approval that was never recorded". This is fail closed: such a run no longer passes its gate.
  To make it decidable, the host calls the plan on the kept state with no `previousState`. That
  files the missing requests (`has_stashed_ids` is false), with no new entry point. The product
  does this once at deploy, with a logged and idempotent script, and the release note says so.
- **A map state kept before R4.** It has no `__mapResults`. Its completed items are rerun once,
  as today; from the next completion on, they keep their results.
- **Every other state** resumes as before: depth 1 is unchanged (R2).

### What the product changes (N8)

Three small product batches, after engine PR 2 and PR 3. Each place below knows the direct
children only:

- **The resume pre-check.** `ensureNoPendingApprovals` reads `[runId, ...directChildRunIds]`
  (`product/apps/api/src/runs/runs-catalog.service.ts`, `runs/domain/subgraph-invariants.ts`). It
  should read the request ids stashed in `__approvalIds`, or the descendant run ids (prefix
  `<runId>:`), filtered by tenant. The engine's refusal holds either way, but the product's rule
  that the `ApprovalEngine` is the source of truth would otherwise be wrong.
- **The proof.** `discoverAndClassifyDescendants` (`run-replay-verification.drizzle.ts`) classes
  every non-direct descendant as `"unsupported"`. After R1 and R2, these descendants have an
  attestation chain to verify.
- **The signer's queue.** Each request carries its item's run id (`run:each:3`). The queue shows
  the item (R2), with personal data masked, and links it to the root run.

### Alternatives

- **Refuse such graphs** (at validation, at publication, or a fail-closed `resume_problems` for an
  unwalked wait, the architect's S2): rejected by the owner on 2026-10-10. A child workflow with
  its own review step, or a review per item of a batch, is a supported shape (ADR 0042, product
  ADR 0068), and refusing it removes the feature instead of governing it. S2 stays a **stopgap**,
  only for what does not ship in the same version: a `mapSubgraph` that holds a gate, as long as
  PR 1 and PR 3 are not both released. The product's published graphs are checked first.
- **One request for the whole fan-out** ("approve all N items"): it cannot say which item a person
  saw. It also contradicts "one approval decides one item" and turns N decisions into one
  signature. Rejected.
- **A resume per item** (item 0 goes on while item 1 still waits): this needs the runtime to
  advance some gates and leave others, which is a new resume entry and a change to the runtime's
  suspension model. Not in this revision.
- **Per-wait stashes** (`__approvalIds` keyed by run id): this would let a resume keep the stash
  of an unchanged item while another moves on. It changes a channel the three bindings and the
  product read. With R3, a coarse clear can at worst file again a tool request that a rejected
  item asks for again. That is noise a person sees, never a gate passed. Deferred.
- **Iterating `0..len(overChannel)`** for map items: same result, but a cost proportional to the
  fan-out width on every plan. Replaced by N1.

### Consequences

- A human gate at any depth, and in each `mapSubgraph` item, waits for a person, and each item
  waits for its own decision.
- The number of requests per wait grows with the fan-out, up to the R5 bound. A signer's queue
  shows one request per item, under the item's run id.
- The TypeScript test `child-workflow-approval.rust.test.ts:186-201` ("NON-REGRESSION: a
  mapSubgraph child's gate is still NOT filed") checks only the parent's run id, so it would stay
  green after the fix while saying the opposite:
  - PR 2 makes it say the truth: ``getPending(`${runId}:sub:0`)`` is empty, a known gap until PR 3;
  - PR 3 flips it to the positive case: one request per item, and a resume refused while one
    waits.
- A map item's side effects run once per item, not once per resume (R4). This is a fix in its
  own right, even without an approval store.
- An agent with an approval-gated tool inside a child workflow fails its run explicitly (R6)
  until the grant-routing revision ships.

### Plan (small batches, each independently revertible)

| PR     | Branch                            | Content                                                                                                                                                                        |
| ------ | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **1**  | `engine/map-results-on-resume`    | R4 in `crates/graph-runtime`: `__mapResults` (N2–N5), `handler_view`; the red test 3 goes green; replay of a journal recorded before R4.                                       |
| **R6** | `engine/child-grant-loop-refused` | R6: `refusal` in the plan (first use of the field), `resume_problems`, a terminal error in the TypeScript and Python SDKs; red then green.                                     |
| **2**  | —                                 | R1 + R2 for `subgraph` nodes at any depth, with the depth bound; the red test 1 goes green; unit and golden cases; TypeScript binding test; the NON-REGRESSION test made true. |
| **3**  | —                                 | R1 for `mapSubgraph` items (N1) + R3 + the request bound + N6 at map entry + `idempotencyKey`; the red test 2 goes green; the NON-REGRESSION test flipped; golden cases.       |
| **4**  | —                                 | Python test over the new golden cases, governance docs page, release note (the deploy script for old checkpoints).                                                             |

PR 1 lands before PR 3, because without R4 the map filing would ask again for items that were
already decided. PR 2 does not depend on PR 1. The product batches (N8) follow PR 2 and PR 3.

### Decisions recorded (owner, 2026-10-10)

1. Bounds: 256 requests per wait and 16 levels, as the engine's ceilings; **beyond, the run
   fails** with the reason; plus the width check at `mapSubgraph` entry when the child holds a
   gate or a gated tool (N6).
2. A rejected gate in one item **ends the run**.
3. The engine shows the **index**; the product shows the item, personal data masked.
4. S2 only for what does not ship in the same version.
5. Child-run tool grants: **explicit refusal now** (R6), then a short revision that routes
   `__approvedTools`.
6. The idempotency key ships with PR 3, without waiting for ADR 0053's train.
7. R4 is accepted with N2–N5; R1 iterates the snapshot keys and refuses out-of-range indexes (N1).

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
