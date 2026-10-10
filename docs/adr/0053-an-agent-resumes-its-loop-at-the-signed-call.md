# ADR 0053 — An agent resumes its loop at the signed call

- Status: **Proposed** — 2026-10-10, revised the same day after the architecture review
  (« VALIDÉ SOUS RÉSERVE »: D1, D2, D3, D5 under reserve; D4, D6 validated — see Revision 1). For
  the owner's decision. Nothing here is implemented. It changes a runtime invariant (what a resume
  of an agent node does), so no code lands before the owner accepts it.
- Date: 2026-10-10
- Deciders: Mathieu (owner)
- Driven by the architect's review of ADR 0051 (2026-10-10: D6 « refusé tel que proposé », §4 « Un
  agent reprend là où il s'est arrêté »), and the owner's decision of the same day: « D6 :
  continuation de l'agent (reprise de sa boucle, exécution exacte de l'appel signé) dans une ADR
  séparée 0053, avec la clé d'effet de l'ADR 0049 D4 dans le même train. Le registre
  `__gatedCalls` est abandonné. » Also bound by the owner's decision 5 of that day (« l'entrée
  canonique reste chiffrée ou en accès restreint ») and by the product's ADR 0068 Revision 10
  (2026-10-10, « un refus de porte termine le run »).
- Relates to: [0025](./0025-unified-agent-middleware-api.md) (the gate, intrinsic to the stack),
  [0032](./0032-secrets-redaction-and-no-log.md) (`noLog`),
  [0038](./0038-replay-as-evidence.md) (the replay journal),
  [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (host tools journaled),
  [0044](./0044-cooperative-run-cancellation.md) (terminal `cancelled`),
  [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first, both SDKs in one release; the
  host node's effect key), [0049](./0049-the-host-keeps-each-checkpoint.md) (D3 a running
  checkpoint re-runs its node; **D4 the effect key, revised here**),
  [0051](./0051-the-signer-sees-and-signs-the-call.md) (`callKey`, `approvalScope: "call"`, D6
  replaced by this ADR), [0052](./0052-a-context-budget-keeps-the-request.md) (a journal mark,
  absence = the old behaviour).

## Revision 1 (2026-10-10) — after the architecture review

The design is unchanged. The review asked for these changes, all made below:

| Review | Change | Where |
|---|---|---|
| K1 | One `handler_view`: strips `__agentResume` at the top level **and** inside every `__subgraphStates[*].channels`; used for node handlers, named conditions, host-node payloads and their input hash | D1 « Who sees it » |
| K2 | The engine masks `__agentResume` itself in events, `explain_run` / `run_insight` and `@ailu-ai/verify` outputs | D1 « Who sees it » |
| K3 | Decision 5 cited; the product encrypts the continuation at rest in its checkpoint sink | D1 « At rest », product changes |
| K4 | Trust model written: the sha256 detects corruption only; optional host-keyed HMAC | D1 « Trust » |
| K5 | Cleared at failure, cancellation and refusal too | D1 « When it goes » |
| K6 | A read-only 2.7.x filter, so that a rollback from 2.8 to 2.7.x is safe | Compatibility, plan |
| K7, K7b | Agent fingerprint over declared fields (tier, not the resolved model), golden vector across versions; messages kept whole, provider blocks included | D1 « Agent fingerprint », « Messages » |
| K8 | `restart`: explicit only, keeps the suspended execution's anchor; owner-only in the product | D2 « An explicit restart » |
| K9 | `memoryWrites` is a per-segment delta, like `usage` | D2 « What the result says » |
| K10 | Optional `resumeMaxAge` per agent, checked by `resume_problems` | D2 « Age » |
| K11 | « Not granted, not refused » happens only with a host that skips the approval check | D2 step 2 |
| K12 | Why the host node's key keeps its form | D3 |
| K13 | `effectKeyOf` reference vectors, in the `callKeyOf` file | D3 |
| K14 | The product's effect table `(tenant_id, effect_key)` is a schema decision for the owner | product changes, Q9 |
| K15 | D4 is a public-API behaviour change: changelog entry with the exact message; scope for direct children | D4 |
| K16 | The journal mark is per node and records what happened | D5 |
| K17 | Verification aggregates every segment; a partial check says so | D5 |
| §6, §7 | `release/2.7` branch; 0053-c split in two; 0053-e eligible for 2.7.0; the owner's decision 4 (spend every key grant) ships only in 2.8.0 with 0053-c/d | Plan |
| §8, §9 | The open questions carry the architect's recommended answers; Q-A–Q-E added | Open questions |

## Context

Read from the source on 2026-10-10: `origin/main` 0644f18 (2.6.1). ADR 0051 D4/D5 are read on
`origin/engine/approval-per-call` (fdfc2b7), marked « branch ».

**What an agent node does at a gate.** The ReAct loop builds a seed message from the input and the
State (`crates/agents-core/src/react.rs:306-330`), runs `before_run` once (`:334-362`), then
iterates (`:364`). For each native tool call of a turn it calls `execute_tool_call` (`:449-466`);
the gate is `MiddlewareStack::before_tool` (`crates/agents-core/src/middleware.rs:241-298`). A
gated call that is not granted pushes an `ApprovalRequestItem` and stops the loop
(`react.rs:584-588`, `break 'iterations` at `:463-465`). The calls after it in the same turn are
never looked at. The node writes the result into its output channel and returns
`NodeOutput::interrupt` (`crates/agents-core/src/node.rs:94-95`); the runtime applies the patch
and suspends (`crates/graph-runtime/src/runtime.rs:994-1001`, `suspend` at `:1178-1193`: one
checkpoint, then `RunSuspended`). The conversation, the iteration count, the tool results and the
pending call are dropped: they lived in `run_scoped`'s locals.

**What a resume does.** The bridge validates the grants and *replaces* `__approvedTools`
(`crates/runtime-bridge/src/lib.rs:512-518`), seeds the checkpoint and calls `resume`
(`:524-526`). A dynamic interrupt re-runs its node (`runtime.rs:647-661`). The agent starts again
from its first LLM call. So:

1. **The model is asked again**, live (ADR 0049 D3). It pays the tokens again and may issue
   another call: the signer approved `refund(A)`, the model now asks `refund(A')`, which runs
   under a name grant (architect R6). What was signed is not what ran.
2. **Calls made before the gate run again.** An ungated `send_email`, or a gated call granted on
   an earlier resume, is executed a second time.
3. **`approvalScope: "call"` does not converge** (architect R9). The host gives back the grants
   of the current wait only (product `approvedToolsFromState` reads `__approvalIds`), so an agent
   that refunds A then waits on B asks for A again; a host that gives back A and B refunds A twice.
   D5's spent set is a set of `callKey` strings in memory for one execution (branch `react.rs:309`,
   `:601`, `:650-656`): it cannot help across a resume, and it loops on « A then A again » — the
   second signature gives back the same string, the re-asked model runs the first A on it again,
   and the second A finds it spent.
4. **A crash has no key.** A host node gets an effect key (`effect_key`,
   `crates/runtime-bridge/src/node_journal.rs:55-62`; payload at `lib.rs:1114-1122`). A host tool
   gets `{ kind, name, input }` only (`lib.rs:1438-1453`, payload at `:1447`). ADR 0049 D4 is not
   shipped, and as written — sha256 of (run, node, version at the node's entry, tool, arguments) —
   it gives two identical, legitimate calls of one execution (two refunds of 40 € on one order)
   the same key: a host that dedupes on it would drop the second.
5. **A `mapAgents` node re-runs every spawn** on resume, finished ones included
   (`node.rs:129-131`).

**Refusals.** The product decided that a refused human or tool gate ends the run (ADR 0068
Revision 10: the run row is `rejected`, a `cancelled` checkpoint and a `run_cancelled` event with
`reason: refused-at-tool-gate`, nothing runs after it). The engine still says « a rejected tool
only stays locked »: `resume_problems` blocks a resume on a rejected `gate:` request only
(`crates/runtime-bridge/src/catalog_approvals.rs:33-37`, `:447-450`).

**What ADR 0051 D6 proposed, and why it is not the answer.** A ledger `__gatedCalls: { callKey →
observation }` written by agent nodes. It needed a node to write an engine-owned channel
(`ENGINE_OWNED_CHANNELS`, `runtime.rs:63-74`, exists so that no node can pre-approve anything);
keyed by `callKey` it merges two identical legitimate calls; it still re-asks the model and feeds
it a recorded observation in a context that may have changed; and it does not cover an ungated
tool with an effect. The owner abandoned it.

## Decision

### D1 — At an approval suspension, the runtime keeps the agent's loop

**The node hands its loop over; the runtime writes it.** A node may attach a *continuation* to an
interrupt: `NodeOutput::interrupt(reason, patch).with_continuation(value)`
(`crates/graph-runtime/src/interfaces.rs:31-59` gains `continuation: Option<Value>`). The runtime
writes it into the engine-owned channel `__agentResume`, under the node's id, in the **same state
mutation** as the interrupt patch (`runtime.rs:994-1001`): one version, one checkpoint, then
`RunSuspended`, exactly as today. `__agentResume` joins `ENGINE_OWNED_CHANNELS`, so a node's patch
(`runtime.rs:985-989`) and a run's input still cannot write it; a node can only hand over its
*own* continuation, keyed by the runtime, and only with an interrupt (a continuation on an update
is dropped). Nothing a node relays can forge another node's transcript.

**What it holds.** The agent node of a ReAct agent fills it (`agents-core`, new
`AgentContinuation`):

| Field | What |
|---|---|
| `anchorVersion` | `state.version` when this execution of the node began its loop (kept across every resume of that execution, and across a restart — D2) |
| `suspendedAt` | the runtime clock at the suspension (the recorded clock, so a replay derives the same value) |
| `iteration` | the loop index reached; `max_iterations` is a budget of the whole execution, not of a segment |
| `callSeq` | how many tool calls this execution has handed to `execute_tool_call` so far |
| `conversation` | the messages the next LLM request would carry: the seed as built and trimmed (ADR 0052), every assistant turn, every observation |
| `pending` | the calls of the stopped turn still to handle, in order — the gated one first, then the ones after it — each `{ id?, name, callInput, callKey, effectKey, callSeq }` |
| `trace`, `todos` | what the final `AgentResult` must still report (`todos` is a « replace » list) |
| `grantedCalls` | the gated calls this execution already executed on a grant (D2) |

`callInput` is the canonical JSON string ADR 0051 (as revised) hashes into `callKey`. The pending
call is executed from it, never from a re-serialized value.

**Messages are kept whole.** `conversation` holds each `LlmMessage` exactly as the loop holds it,
every field serialized: `content`, `content_blocks`, `tool_calls` with their ids, `tool_call_id`,
`tool_name`. So the first request after a resume is the request the uninterrupted loop would have
sent: a provider sees the same `tool_use` / `tool_result` pairing, and a replay finds the
journaled request. Today the gateway has no signed-reasoning block (`ContentBlock`,
`crates/llm-gateway/src/types.rs:92-116`) and the loop keeps no response blocks on an assistant
tool-call turn (`react.rs:441-448`). When the loop keeps such blocks (signed thinking, web search
results), they are in the continuation by construction; the test suite carries a resume on the
Anthropic adapter with `tool_use` (and with thinking, once the gateway carries it).

**On the wire it is one opaque string.** `__agentResume[nodeId] = { "v": 1, "agent":
"<agentDigest>", "body": "<canonical JSON of the table above>", "sha256": "<hex of body>",
"hmac"?: "<hex>" }`. The host keeps the checkpoint (ADR 0049) and may store it as `jsonb` and read
it back with `JSON.parse`, which turns `40.0` into `40` and rounds an integer above 2^53 (architect
R3). A string crosses that unchanged, so the restored conversation — and the pending input — are
byte for byte what the model wrote.

**Agent fingerprint.** `agent` is sha256 of the canonical JSON of the agent's **declared** shape, as
the graph states it: name, `provider`, the declared `model` (or `null`), the declared `tier` (or
`null`) — never the model the tier resolved to —, `system` as given in the spec, `max_iterations`,
`approvalScope`, and each tool's name, `requires_approval`, `content_scoped` and conditions. It is
not a digest of the whole spec, so an engine upgrade that adds a defaulted spec field, or changes
the tier table, does not invalidate a waiting continuation. Tool descriptions and schemas are out:
they change what the model is told next, not whether the recorded loop is still this agent's. A
golden vector of the digest is checked by every release, Rust and both SDKs. A host that composes
part of the system prompt at run time (the product's policies, skills) must pin the composed text
at run start, as the product already pins its run environment (product ADR 0068 D6); otherwise a
policy changed during the wait reads as « agent changed » (D2 step 1), which is the safe reading.

**Size.** The body is at most the agent's next LLM request plus the pending calls, so it is bounded
by the model's context window (and by the context budget, ADR 0052, when one is set). A run in
record mode already journals every such request; a suspended agent adds one more to its checkpoint.
A hard cap guards the host: `AGENT_RESUME_MAX_BYTES`, 4 MiB by default, which a host may set on the
run spec. Over it, the node fails (`FailureCategory::Permanent`, `agent_resume_too_large: <bytes>;
resume with agentResume: "restart" to start the agent again`) instead of suspending: fail closed,
because a suspension that cannot resume exactly is the double effect this ADR removes.

**Who sees it.** No node does, except its owner. One function, `handler_view(state)`, builds the
state every host-facing reader gets: it removes `__agentResume` at the top level **and** inside each
`__subgraphStates[*].channels` — a suspended child's state is copied whole into its parent
(`set_subgraph_state`, `runtime.rs:283-298`), continuation included. `handler_view` serves node
handlers (where today `with_injected` builds the handler's copy, `runtime.rs:375-386`), named
conditions (`host_condition`, `lib.rs:1137-1138`) and host-node payloads (`host_node_payload`,
`lib.rs:1114-1122`), whose input hash (`hash_node_input`, `node_journal.rs:50-53`) is taken over
the same view, so a replay hashes what the record hashed. The resumed node finds its own
continuation under the per-execution key `__resume`, never persisted. Without this, the seed State
of every later agent — the seed serializes every channel, `react.rs:309-322` — would carry other
agents' transcripts, and a child's transcript would reach its parent's handlers.

**Masked by the engine.** `__agentResume` is declared in no `ChannelDefinition`, so `mask_no_log`
(`runtime.rs:97`) would not see it. The engine masks it itself, with the `noLog` sentinel, in
everything it produces: run events, `explain_run` and the rest of `run_insight`
(`crates/runtime-bridge/src/run_insight.rs:180`), and the outputs of `@ailu-ai/verify`; at every
depth of `__subgraphStates`. The interrupt path already emits no `NodeCompleted`. A host still
treats it as a `noLog` channel (ADR 0032) wherever it exports or shows state itself.

**At rest (the owner's decision 5).** The continuation holds `callInput` and the seed (`noLog`
channels included, as the prompt did) and tool results. Decision 5 says the canonical input stays
encrypted or restricted. The engine hands the checkpoint to the host's sink (ADR 0049 D1); the
host encrypts the **value** of `__agentResume` there and decrypts it before a resume. The opaque
form makes this a string-to-string step: the host gives back the exact string, so `sha256` (and
`hmac`) still match. The product's part is listed below.

**Trust.** The checkpoint store is part of the trusted base: `__agentResume` lives there, next to
`__approvedTools`, which a resume without `approvedTools` does not rewrite (`lib.rs:512-518`). The
`sha256` is **inside** the object: it detects corruption, not forgery. Whoever can write a
checkpoint can change the conversation (forged observations) or `pending` (one more ungated call)
and recompute it — and a continuation, unlike a grant, makes calls run without the model. So the
engine offers a keyed integrity check, optional: with a host key (`AILU_CONTINUATION_KEY` for the
bridge, or a `continuationKey` option), the runtime writes `hmac = HMAC-SHA256(key, agent ‖ body)`
and D2 step 1 requires it to match. Without a key, the engine trusts the store, and says so. A
gated pending call still passes the gate against the grants of the resume, so a forged continuation
cannot unlock a gated call; the HMAC is what protects the ungated ones.

**When it goes.** The runtime clears `__agentResume[nodeId]` in every state mutation that ends the
node or the run: the node's completion (`runtime.rs:1033-1045`), its error routing (`:941`), a run
failed after its retries (`:971`) or by `fail_run` (`:1577-1582`), and a cancellation at a node
boundary (ADR 0044, `:825-836`). Each removal is part of a checkpoint that is written anyway. A
host that ends a suspended run itself writes its terminal checkpoint without it (the product's
`RunRefusal` and its cancel of a suspended run). A segment's replay starts from that segment's
first checkpoint, never from a terminal one, so nothing needs the transcript once the run is over.

### D2 — On resume, the agent continues at the pending call and executes exactly that call

When the node runs and finds `__resume`, it does not build a seed, does not run `before_run`, and
does not call the model. It checks, then continues:

1. **Checks, fail closed.** `v` is known; `sha256(body)` matches; `hmac` matches when a key is
   set; `agent` equals the digest of the agent the graph now declares; each pending call's
   `callKey` and `effectKey` recompute from its `callInput` and the anchor (D3). Any failure fails
   the node (`Permanent`, `agent_resume_refused: <which check>`), so `NodeFailed`, then `RunFailed`
   or the node's error edge. Before the resume, `resume_problems` reports the same reasons, so a
   host refuses it with a message rather than a failed run.
2. **The pending calls, in order.** Each goes through `before_tool` — the built-in gate and the
   installed middleware — against the `__approvedTools` the resume wrote, with `ToolCallCtx` now
   carrying the call's `effectKey`. A gated call may not have its input rewritten by a middleware
   (architect R2, `ToolControl::Deny`): the gate decides on the input that runs.
   - **Granted:** the handler runs with the parsed `callInput`; its observation is appended under
     the call's `id`, as `execute_tool_call` does (`react.rs:604-639`). It is recorded in
     `grantedCalls` (`{ callKey, effectKey, grant, resumed: true }`).
   - **Not granted, not refused:** the node suspends again with the same request and a
     byte-identical continuation. No LLM call is made. The suspension key is the same (D3), so the
     host files nothing new. This only happens with a host that resumes without checking the
     approvals: `resume_problems` already refuses a resume whose request is « still pending ». It
     is the fail-closed answer, not a path a governed host takes.
   - **Refused:** the run does not get here (D4).
   - A later call of the same turn that needs another gate suspends again on that call, with a new
     continuation.
3. **Then the loop goes on** from `iteration`, with the next LLM request: the conversation plus the
   observations just appended. That request is a new step, never a repeat of one already answered.
   `before_model`, `after_model`, `on_iteration` and `after_run` run as in any iteration.

Under `approvalScope: "tool"` (the default) the call that opened the request is the one that runs,
with the arguments the signer saw; later calls of that tool in the execution still run on the name
grant, as ADR 0051 says. Under `"call"`, every gated call runs on its own grant, once. An agent that
makes N gated calls is suspended N times and each signed call runs once: R9 is closed.

**What the result says.** `AgentResult` gains `grantedCalls` (additive). `reasoning` is the trace of
the whole execution. `usage`, `webSearch` and `memoryWrites` are **this segment's**: the LLM calls,
searches and memory writes made since the resume, as today (each resume used to rebuild them from
zero). A host that bills or drains each segment counts each item once, with no deduplication.
`todos` is the whole execution's list, since a todo list replaces the previous one.

**An explicit restart.** A resume may pass `agentResume: "restart"`, for one node or for all (TS
`ResumeCatalogGraphOptions.agentResume`, Python `agent_resume=`, the C-API spec): the agent starts
from its first LLM call, the 2.6 behaviour, with what that means — calls already made are made
again, and the model may diverge. It is never an automatic fallback; it is the way out when step 1
refuses a continuation (the agent was changed, the cap, the age), and the operator's switch if the
continuation misbehaves. Before dropping the continuation, the runtime reads its `anchorVersion` and
hands it to the restarted execution (in `__resume`, marked as a restart): calls the model makes
again identically and at the same rank get the **same** `effectKey`, so a host that keys its effects
on it does not repeat them. The restart is recorded in the journal (D5). In the product, only the
owner role may restart an agent, the action is written in the run's journal, and the screen warns of
a possible double execution (Q-D).

**Age.** An agent may declare `resumeMaxAge` (a duration, optional). The engine reads no clock at
resume; `resume_problems` takes the host's `now` and compares it with the continuation's
`suspendedAt`: past the age, it reports « the agent waited longer than <age>: have the call signed
again, or restart it ». A resume days later otherwise runs the signed call on old observations (a
balance read before a refund) — which is what was signed, but needs a bound (Q-B).

**Middleware state.** A middleware's in-memory state is not part of the continuation. The one
built-in that keeps any, `StructuredOutputMiddleware::last_valid` (`middleware.rs:567`), is read in
`after_run`, which runs after the resume. `before_run` middleware (memory recall, brain, skills,
budget) are not re-run: the agent continues with the context it was given. A host that wants the
agent to see what changed while it waited restarts it.

### D3 — A call has an occurrence identity: the effect key (revises ADR 0049 D4)

`callKey` says *what* is called (name and arguments). The **effect key** says *which* call:

```
effectKey = sha256( canonical JSON of
  ["tool", logicalRunId, nodeId, anchorVersion, spawn, callSeq, callKey] )
```

`logicalRunId` drops the replay-fork segments (`logical_run_id`, `node.rs:280-297`), so a replay
derives the keys of the recorded run. `spawn` is the `mapAgents` index, `null` for an agent node.
`callSeq` is the call's ordinal in the execution. `anchorVersion` is the version at which the
execution began, carried by the continuation, so a call keeps its key across a suspension, a resume,
a crash re-run and a restart. It is an array, like the host node's key (`node_journal.rs:55-58`), so
no field can collide with another by containing a separator.

This replaces ADR 0049 D4's formula before it ships: the same goal, plus `callSeq` (two identical
calls of one execution get two keys) and the anchor (a resumed call keeps the key it was filed
under).

**The host node's key keeps its form** (`[runId, nodeId, version]`, raw run id, ADR 0045 D1.2). It
has shipped since 2.2.0 and hosts store it: changing its value would make a retry in flight across
an upgrade look like a new effect. A host node is never called by a replay, so the raw run id costs
nothing there. The `"tool"` prefix keeps the two key spaces disjoint. Should the host node's key
ever move to `logicalRunId`, that is a value change with its own journal mark, not part of this
ADR.

**Reference vectors.** `effectKeyOf(...)` is exported by napi and Python from the one Rust
implementation, with shared vectors in the same file as `callKeyOf`'s (owner's decision 1): a
repeated call (two `callSeq`), `spawn` `null` and `0`, `callSeq` `0`, run ids with `:fork:<n>` in
the middle and at the end, each tested in Rust, TypeScript and Python.

**Where it goes.**

- On every `ApprovalRequestItem` and catalog `ApprovalSubject` (`effectKey`, additive).
- On every tool invocation: the host payload becomes `{ kind: "tool", name, input, callKey,
  effectKey, resumed }` (`lib.rs:1447`); TS `RustToolBinding.execute(input, context)` gets it as a
  second argument (`packages/graph-sdk/src/rust-engine.ts:540`, `:638-642`); Python accepts a
  `HostTool` whose `execute` takes a `HostToolInput`, next to plain callables
  (`python/ailu/__init__.py:848-852`); native Rust tools registered with
  `InMemoryToolRegistry::register_with_context` get a `ToolContext`. Built-in native tools (todos,
  memory intents, the governed fs) act inside the run and need none.
- In the tool journal (`ToolResultWire.effectKey`, additive) and in `grantedCalls`.

**How two identical legitimate calls are told apart — everywhere by `effectKey`.**

- *Filing.* `suspension_key` (`catalog_approvals.rs:350-363`) reads, per request, `effectKey`, else
  `callKey`, else `approvalKey`, else `subject`. The R1 fix (ADR 0051 review) introduces the
  `callKey` tier and the rule « a resume whose stored approvals were all decided and that suspends
  again files again »; the `effectKey` tier makes it exact: the second `refund(A)` is a new request,
  a re-suspension on the same pending call is not.
- *Grants.* `ApprovedTool` (`crates/runtime-bridge/src/spec.rs:239`) gains `effectKey`. The bridge
  then writes `<callKey>@<effectKey>` into `__approvedTools` (validated like a content-scoped key,
  `validate_approved_tools`, `lib.rs:590`), and `resume_problems` matches it against the stored
  request's `effectKey`. Such a grant unlocks that occurrence and nothing else, whatever the host
  gives back later: no spent set is needed, and ADR 0051 D5's residual (a later execution of the
  node, between two resumes) cannot use it. A grant without `effectKey` keeps ADR 0051 D5: spent by
  its call within the segment.
- *Effects.* A host tool that acts outside the engine keys its effect on `effectKey` and returns
  the recorded outcome for a key it has seen.
- *Proof.* D5.

**At most once per key, at least once delivered.** The engine checkpoints at node boundaries, not
inside the loop (product rule 120: no checkpoint per tool call on the hot path). So:

| Crash | On re-run (ADR 0049 D3) | Effect |
|---|---|---|
| Before the suspension checkpoint | the loop starts again; the model is asked again | same key if the model repeats the call at the same place; otherwise a new key — the engine cannot do better |
| While suspended | nothing ran | — |
| After the resume checkpoint (`runtime.rs:688`), during or after the pending call | the continuation is restored; the pending call is delivered again with **the same key** | the host returns the recorded outcome |
| After the pending call, during later live turns | the pending call is delivered again with the same key, then those turns are asked again | the pending call: as row 3; the later calls: as row 1 |
| After `NodeCompleted` | done | — |

The engine never claims exactly once. A tool that ignores the key may act twice.

### D4 — A refused gate ends the run; the engine never answers the model with a refusal

- `resume_problems` treats a rejected `tool:` request like a rejected `gate:` request (the module
  docs at `catalog_approvals.rs:16`, `:33-37` and the rule at `:447-450` change). The problem reads,
  exactly: `request <id> (tool:<name>) was rejected by <resolvedBy>` — the wording a rejected gate
  already gets, with `a reviewer` when the store names no resolver. Every resume entry that checks
  approvals (TS, Python, C-API) then refuses.
- Scope: the requests the engine checks, which include a direct child's, filed under the child's
  run id and stashed in the parent's `__approvalIds` (module docs, `catalog_approvals.rs:8-13`). A
  rejected `tool:` request in a direct child blocks the parent's resume, as a rejected `gate:` does.
  A nested child, and a `mapSubgraph` fan-out's children, stay unwalked, as today.
- **A public-API behaviour change** for every host of the open-source engine: until now a rejected
  tool « stayed locked » and the resume went on. It goes in the changelog's « Behaviour » section,
  with the message above.
- The host ends the run its way. The product does it (ADR 0068 Revision 10: `cancelled` checkpoint,
  `run_cancelled` with `reason: refused-at-tool-gate`). The engine vocabulary does not change: no
  `rejected` status, no `run_rejected` event.
- No « rejected » observation is fed to the model, and the continuation is never resumed after a
  refusal: nothing runs after it, no LLM call.
- It does not need the continuation: it can ship in 2.7.0 (plan).

### D5 — Replay and verification re-derive it

**A journal mark per node, recording what happened** (precedent: ADR 0052 D2). `ReplayJournalWire`
(`lib.rs:153-169`) gains `agentResume: { "<nodeId>": "continue" | "restart" }`. A recording run
writes, for each agent node whose first execution in the segment is a resume, what that execution
actually did: `"continue"` when it restored a continuation, `"restart"` when it found none (a
checkpoint from before 0053) or was told to restart. A node absent from the map, and a journal
without the field (recorded before 0053), replay as `"restart"`: the agent starts from its first
LLM call, as it did. An unknown value fails the replay (`invalid replay_journal JSON`). A replay
never infers « continue » from the presence of a continuation in the state; it does what the mark
says, so a run that resumed two agents — one with a continuation, one without — replays both right.

**Segment by segment.** A suspended-then-resumed run is replayed one segment at a time, each from
its own starting checkpoint with its own journal (ADR 0052, Consequences).

- The segment that suspends re-derives the same continuation: it is a pure function of the
  journaled LLM responses, the journaled tool results and the state at the node's entry.
- The segment that resumes restores the continuation from its starting checkpoint. The pending host
  call is served from the tool journal by `(name, inputHash)` (`lib.rs:296-350`), and its recorded
  `effectKey`, when the journal has one, must equal the re-derived one, or the replay fails
  (`tool_effect_mismatch`). The next LLM request matches the journaled one, since the conversation
  is restored byte for byte.

**Signed is executed, over the whole run.** `run_insight` gains `verify_granted_calls(attested,
segments)`. It aggregates `grantedCalls` over every replayed segment of the run, every execution of
a node in a graph loop, and every agent, in order. Every approved attested record that carries a
`callKey` must match a `grantedCalls` entry with the same `callKey` (and the same `effectKey` when
both carry one), and every entry with a per-call grant must match an approved record. A mismatch
reads « signed X, executed Y ». So the proof capsule carries **each segment**: its starting
checkpoint (which holds the continuation, so the masking and encryption of D1 apply to the capsule)
and its journal. A capsule that lacks a segment gets a partial verdict that says so — `partial`, « 2
of 3 segments verified » — never `ok`. `@ailu-ai/verify` runs it next to `verifyReplayDecisions`,
taking the attested side from `capsule.attestation.records` (architect R8,
`packages/verify/src/verify.ts:116-117`); Python exposes it through the native call. The attestation
view may carry `effectKey` behind the same opt-in as `callKey` (ADR 0051 D2): a record without it is
unchanged byte for byte.

### D6 — `mapAgents`: the same, per spawn (decided here, shipped after the agent node)

The map node's entry holds one continuation per suspended spawn and the result of each finished
one: `{ "v": 1, "spawns": { "<index>": <continuation> | { "done": <result> } } }`. On resume only
the suspended spawns continue; a finished spawn's result is reused, not re-run (`node.rs:129-131`
becomes false). Spawn events stay as ADR 0050 defines them. Until it ships, `approvalScope: "call"`
on a `mapAgents` sub-agent is refused at compile with a typed error, in both SDKs (as architect R10
asks for the TS agent), and a map node resumes as today.

## How the runtime invariants hold

| Invariant | Holds because |
|---|---|
| Deterministic by default | The continuation is a pure function of recorded inputs (`suspendedAt` comes from the recorded clock); restore is a parse; `effectKey` and `callKey` are pure functions of recorded data. A replay does what the per-node journal mark says (D5). A live resume makes fewer LLM calls, never different ones for steps already answered. |
| Checkpoint after every node completion and state mutation | The continuation is written in the interrupt's mutation (one checkpoint, as today) and cleared in the mutation that ends the node or the run (one checkpoint, as today). No new mutation, no checkpoint inside the loop. |
| An event per lifecycle transition | Suspension: `RunSuspended`. Resume: `RunResumed` (`runtime.rs:682`) then `NodeStarted` (`:865`). Completion: `NodeCompleted`, whose `agentResult` carries `grantedCalls` (masked by `noLog` as before). A refused restore: `NodeFailed`. No transition is added, so no event is added; `__agentResume` is masked in all of them. |
| Gates suspend cleanly and resume from the latest checkpoint | The suspension is unchanged. A resume loads the latest checkpoint (`runtime.rs:642-646`), the continuation is in it. A resume without a decision suspends again on the same request, without an LLM call. |
| No self-approval | Unchanged: the pending call passes the same gate against the grants the bridge validated (`lib.rs:512-518`). A continuation cannot grant anything. |
| Tenant isolation (rule 120) | The continuation lives in the run's checkpoint, never in a parent's handler view. `effectKey` contains the run id; the product still indexes it with `tenant_id`. |

## Compatibility

- **Old checkpoints.** A checkpoint suspended before 0053 has no `__agentResume`: the agent starts
  from its first LLM call, as in 2.6, and the journal says `"restart"` for it. `approvalScope:
  "call"` is not adopted before 0053 ships (owner's decision 3), so no such run is waiting on a
  per-call grant.
- **Old journals** replay as recorded (D5).
- **Rollback.** An engine without 0053 sees `__agentResume` as an ordinary channel: its agents would
  put it in their seed State, and the transcripts would go to the LLM provider. So the 2.7 line gets
  a read-only filter that strips `__agentResume` (at every depth of `__subgraphStates`) from seeds,
  handler views, events and `run_insight` — no behaviour change, since no 2.7 engine writes it. It
  ships in 2.7.0 if ready, else in a 2.7.x cut from `release/2.7`. A rollback from 2.8 goes to that
  2.7.x or later, never below; a waiting run then restarts its agent (2.6/2.7 semantics) without
  leaking.
- **SDK parity (ADR 0045).** Every graph of `@ailu-ai/graph-sdk` runs on the Rust engine: the
  TypeScript engine fallback is gone (`packages/graph-sdk/src/compiled-graph.ts:128-148`,
  `:216-222`). Python runs the same bridge (`python/ailu/__init__.py:1253`), and so does the C-API.
  They get D1–D5 in one release, with the surface additions listed below. The legacy TypeScript
  `GraphRuntime` (`packages/graph-runtime/src/runtime.ts:85`, `DynamicInterrupt` at `:422`) and the
  standalone TS `ReActAgent` (`packages/agents-core/src/react-agent.ts:223-233`) keep their
  contract: no checkpoint of their own, so no continuation; the caller re-runs with granted names.
  They refuse `approvalScope: "call"` with a typed error (architect R10), and their docs say so.
- **Public API, all additive on the wire.**
  - Rust `graph-runtime`: `NodeOutput.continuation`, `NodeOutput::with_continuation`,
    `AGENT_RESUME_KEY`, `handler_view`. Rust `agents-core`: `AgentContinuation`,
    `ReActAgent::run_resumable` / `ReActAgent::resume_from`, `ApprovalRequestItem.effect_key`,
    `AgentResult.granted_calls`, `ToolContext`, `InMemoryToolRegistry::register_with_context`,
    `effect_key_of`, `agent_digest`; `ToolCallCtx.effect_key` (a new public field: code that builds
    a `ToolCallCtx` by hand must add it). Bridge: `ApprovedTool.effect_key`,
    `ToolResultWire.effect_key`, the journal's per-node `agentResume`, the resume option, the
    continuation key option, `resumeMaxAge` and `now` in `ResumeCheckInput`, `verify_granted_calls`.
  - TypeScript: `ResumeCatalogGraphOptions.agentResume`, the tool context argument, `effectKey` on
    pending approvals, `AgentResult.grantedCalls`, `verifyGrantedCalls`, `effectKeyOf` (napi, one
    implementation, like ADR 0051's `callKeyOf`).
  - Python: `agent_resume=`, `HostTool` / `HostToolInput`, `verify_granted_calls`, `effect_key_of`.
- **Behaviour changes**, in the changelog's « Behaviour » section: a resumed agent makes fewer LLM
  calls and repeats no call; its iteration budget covers the whole execution (an agent that used to
  get a fresh budget at each resume no longer does); a rejected tool request blocks the resume (D4,
  with its exact message); `memoryWrites` of a resumed segment holds that segment's writes.
- **Versioning.** Minors. Proposed: **2.7.0** ships the R1 fix, ADR 0051 D1–D3 as revised, D4
  here (0053-e) and the rollback filter if ready; **2.8.0** ships the rest of this ADR, ADR 0049
  D4 as revised here, and ADR 0051 D4/D5 together (D4 is only safe with this ADR, architect R9).

## Alternatives considered

1. **The `__gatedCalls` ledger (ADR 0051 D6).** Rejected by the owner, for the reasons in Context:
   a node writing an engine-owned channel, `callKey` merging identical calls, the model asked again
   and served an observation out of its context, ungated effects not covered.
2. **Ask the model again (today).** Simple, but it costs the tokens, lets the model diverge from
   what was signed (R6), repeats every earlier call, and makes `"call"` loop (R9).
3. **Ask the model again, served from the previous segment's journal.** No new tokens for the
   prefix, but it needs record mode on every run (`AILU_LLM_RECORD`), the host must keep and pass
   the journal, the prefix's tool calls must be served too, and the seed is rebuilt from a State
   that changed (`__approvedTools` is in it), so the first request no longer matches the journal.
   Strictly more moving parts than storing the conversation.
4. **The agent as a graph of steps** (one node per LLM turn and per tool call, checkpointed each).
   True per-step durability, and the crash rows of D3 would close. But a checkpoint, awaited by the
   host (ADR 0049 D1), per tool call is the hot-path cost rule 120 forbids without an ADR, and it
   changes the event vocabulary. Not now; it can come as a durability tier later.
5. **The continuation in the agent's output channel** (`agentResult.continuation`). No runtime
   change, but that channel is not engine-owned: any node, or an LLM's JSON relayed by one, could
   write a forged conversation and pending call that the agent would then continue with authority.
6. **The architect's « minimal D6 »** (the host gives back every signed, unexecuted per-call grant;
   the runtime removes spent grants from `__approvedTools`). It stops the loop but not the model's
   divergence nor the repeated earlier calls. D3's bound grants give the same guarantee without a
   node narrowing an engine-owned channel; the architect agrees (review §3).
7. **On a refusal, tell the model and continue.** Contradicts ADR 0068 Revision 10: nothing runs
   after a refusal.
8. **A digest of the whole agent spec, or of the resolved model.** Simpler to compute, but any
   defaulted field added by an engine release, or a change of the tier table, would refuse every
   waiting continuation after an upgrade. The declared fields are what the graph promised.

## Consequences

- **What is signed is what runs**, under both scopes, for the call that opened the request; under
  `"call"`, for every gated call. `verify_granted_calls` makes it checkable by a third party, over
  the whole run.
- **Cost.** One LLM call fewer per gated call per resume, and no repeated tool calls. One string the
  size of one LLM request per suspended agent in its checkpoint, gone when the node or the run ends.
- **Data protection.** The checkpoint of a suspended agent holds its conversation: the seed (State,
  `noLog` channels included, as the prompt did), tool results, model output. It was already in the
  model journal of a recorded run. The engine keeps it out of every handler view and masks it in
  everything it outputs; the host encrypts it at rest; a proof capsule that must replay a resume
  segment carries it, so the capsule's retention and access rules apply.
- **Trust.** Without a host key, the checkpoint store is trusted with the transcript as it already
  is with grants; with one, a tampered continuation is refused.
- **Host work.** To get at-most-once effects, a host tool keys its effect on `effectKey`. To bind a
  signature to one occurrence, the host stores the request's `effectKey` and gives it back with the
  grant.
- **Failure modes made loud.** A continuation over the cap, too old, tampered with, or that does not
  match the agent, fails the check instead of resuming differently; the owner can restart the
  agent on purpose.

## What the product changes to consume it

1. Bump to 2.8.0 (and, before that, to the 2.7.x that carries the rollback filter, so that it is the
   rollback floor).
2. **Encrypt `__agentResume` at rest** (decision 5): in its checkpoint sink (ADR 0049 D1), the
   product replaces the value of `__agentResume` by its ciphertext and restores it before a resume,
   with AES-256-GCM as `encryptSecret` does for `kb_imports.pending_content`
   (`product/apps/api/src/connectors/connectors.crypto.ts`; a single cluster key today — a key per
   tenant is the product's choice). Treat it as `noLog` in its own checkpoint reads and exports.
3. **Set the continuation key** (`AILU_CONTINUATION_KEY`, in the cluster's secrets) so a tampered
   continuation is refused (Q-A).
4. Store `effectKey` with each approval request (it is in `subject`); give it back in
   `approvedTools` on resume.
5. **An effect table** `(tenant_id, effect_key) → outcome` for connector writes (refund, payout),
   `tenant_id NOT NULL`, a composite index, a retention: keyed on the tool context's `effectKey`,
   returning the recorded outcome for a key already seen. **A schema change: Mathieu's review
   first** (Q9). The product never looks up by `effect_key` alone.
6. Restart is owner-only, journaled, with a warning on screen (Q-D); `resumeMaxAge` for the tools of
   levels 4 and 5 (Q-B).
7. Then set `approvalScope: "call"` on the agents whose gated tools move money or data.
8. Show « 1 signature, 1 appel » from `grantedCalls`; run `verifyGrantedCalls` over every segment in
   verify-replay, and export each segment in the capsule.

## Implementation plan (small batches, ADR 0048 DORA)

One concern per branch, each green on its own (`cargo test` of the crate, the SDK's typecheck, lint
and tests, the golden and `*.rust.test.ts` suites it touches) and revertible alone. In this order:

1. **R1 fix** — `suspension_key` reads `callKey`; « all decided and suspended again → file again »;
   catalog-path tests A then B, A then A, two conditioned calls (also fixes ADR 0046). Before #328.
2. **#327 revisions** — ADR 0051 D1–D3 with R2–R8, `callInput`, `callKeyOf`, the effective grant in
   the signed view.
3. **0053-e, refusal** (D4) and **the rollback filter** (Compatibility) — both behaviour-neutral for
   continuations, both eligible for 2.7.0.
4. **Release 2.7.0** from `main`, then create **`release/2.7`** at the tag: every 2.7.x fix is cut
   from there, because `main` will carry #328 unpublished (Q-C).
5. **#328** rebased on 1, merged into `main`, not published (owner's decision 3), with R9's
   documenting test and R10's typed refusal. The owner's decision 4 (every key grant is spent by its
   call, under `"tool"` too) **does not ship in 2.7.x**: without the continuation and bound grants
   it makes conditioned grants (0046/0048) loop on « A then A » (review §6). It ships in 2.8.0 with
   0053-c and 0053-d.
6. **0053-a, runtime seam** (`graph-runtime` only): `NodeOutput.continuation`, `__agentResume`
   engine-owned, written at the interrupt, cleared at completion, error routing, failure and
   cancellation, `handler_view` (top level and `__subgraphStates`, handlers, conditions, host-node
   payloads and hashes), engine-side masking, `__resume` for its owner. No node uses it yet: no
   behaviour change. Test: a suspended child holding an agent leaks nothing to its parent.
7. **0053-b, the effect key** (Rust): `effect_key_of` and its shared vectors, `anchorVersion` and
   `callSeq` in the loop, `effectKey` on requests, host-tool payloads and the tool journal, the
   `effectKey` tier of `suspension_key`. Amends ADR 0049's status line (D4 revised by 0053).
8. **0053-c1, the continuation in `agents-core`**: `AgentContinuation`, `run_resumable`,
   `resume_from`, `agent_digest` and its golden vector, the restore checks (sha256, HMAC, digest,
   keys) — pure functions, unit-tested.
9. **0053-c2, the continuation in the node and the bridge**: the node handler, the cap, the per-node
   journal mark, `agentResume: "restart"` keeping the anchor, `resumeMaxAge` in `resume_problems`.
   Tests: A, B, C gated in sequence (each executed once, three requests, model called once per
   turn); A then A again; two gated calls in one turn; a resume without a grant suspends again
   identically with zero LLM calls; a pre-0053 checkpoint restarts and is marked so; a crash after
   the resume checkpoint redelivers the same `effectKey`; replay with and without the mark, and a
   run resuming two agents, one of each; a changed agent is refused, then restarted with the same
   keys for identical calls; a resume on the Anthropic adapter with `tool_use`.
10. **0053-d, bound grants**: `ApprovedTool.effectKey`, `<callKey>@<effectKey>` in
    `__approvedTools`, `resume_problems` matching; then the owner's decision 4.
11. **0053-f, SDK surfaces**: TS and Python options, tool context, `HostTool`, `effectKeyOf`, types,
    parity tests in both SDKs and the C-API.
12. **0053-g, verification**: `verify_granted_calls` over segments with its `partial` verdict,
    `@ailu-ai/verify` (with R8) and capsule segments, the Python docstring, `effectKey` in the
    attestation view behind the opt-in.
13. **Release 2.8.0**: publishes 0053, ADR 0049 D4, ADR 0051 D4/D5 and decision 4. Then the product
    (above).
14. **0053-h, `mapAgents`** (D6), in a later minor.

## Not decided here

- Checkpointing inside the loop (alternative 4), or any durability tier.
- An engine entry that ends a refused run itself (the product's `RunRefusal` does it today).
- Showing the continuation to a signer (« the agent's reasoning before this call »): product UX.
- Moving the host node's effect key to `logicalRunId` (D3).

## Open questions for the owner

Each with the architect's recommended answer (review §8, §9).

1. **The design** — continuation handed over by the node, written by the runtime in `__agentResume`,
   one opaque string per node. *Recommended: yes, with the subgraph filter, the engine-side masking
   and encryption at rest (K1–K3).*
2. **ADR 0049 D4 revised** — the effect key gains `callSeq` and the anchor version, and becomes the
   occurrence identity of filing, grants and proofs. *Recommended: yes, before it ships, with the
   shared vectors (K13); 0049's status line amended in 0053-b.*
3. **Over the cap** — fail the node (fail closed) or suspend without a continuation (the old
   re-run)? *Recommended: fail, 4 MiB by default, settable by the host, with a message that offers
   `restart`; measure the size distribution on the beta before 2.8.0.*
4. **A changed agent** — refuse the continuation and let the host pass `agentResume: "restart"`?
   *Recommended: yes, with a digest of the declared fields (K7); the restart keeps the anchor (K8)
   and is owner-only in the product. No global flag.*
5. **Bound grants** — give the signature's occurrence back (`effectKey` on `ApprovedTool`) rather
   than have the runtime remove spent grants from `__approvedTools`? *Recommended: bound grants —
   stronger, and no node narrows an engine-owned channel.*
6. **`usage` per segment** (as today) rather than for the whole execution? *Recommended: yes, and
   `memoryWrites` likewise (K9).*
7. **`mapAgents`** later, with `"call"` refused on its sub-agents until then? *Recommended: yes, a
   typed compile error in both SDKs.*
8. **Release order** — 2.7.0 cut before #328 merges; 0053-e in 2.7.0? *Recommended: yes to both,
   with `release/2.7` for fixes, the rollback filter in 2.7.x, and decision 4 kept for 2.8.0.*
9. **The product's effect table** `(tenant_id, effect_key) → outcome` (schema change, K14)?
   *Recommended: yes, `tenant_id NOT NULL`, composite index, a retention, reviewed before any
   migration.*
10. **Q-A — An HMAC of the continuation by a host key**, on top of the sha256? *Recommended: yes for
    the product (key in the cluster's secrets); optional in the engine.*
11. **Q-B — A maximum resume age per agent?** *Recommended: yes, optional; the product sets it for
    the tools of levels 4 and 5 (24 h, say); past it, the call is signed again.*
12. **Q-C — A `release/2.7` branch for fixes** once #328 is merged unpublished? *Recommended: yes.*
13. **Q-D — Who may restart an agent in the product?** *Recommended: the owner role only; the action
    is attested in the run's journal and the screen warns of the double-execution risk.*
14. **Q-E — `memoryWrites` as a per-segment delta?** *Recommended: yes* (adopted in D2, pending the
    owner).
