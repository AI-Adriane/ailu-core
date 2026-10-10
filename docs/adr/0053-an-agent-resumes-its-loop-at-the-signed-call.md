# ADR 0053 — An agent resumes its loop at the signed call

- Status: **Proposed** — 2026-10-10, for review by the architect, then the owner. Nothing here is
  implemented. It changes a runtime invariant (what a resume of an agent node does), so no code
  lands before the owner accepts it.
- Date: 2026-10-10
- Deciders: Mathieu (owner)
- Driven by the architect's review of ADR 0051 (2026-10-10: D6 « refusé tel que proposé », §4 « Un
  agent reprend là où il s'est arrêté »), and the owner's decision of the same day: « D6 :
  continuation de l'agent (reprise de sa boucle, exécution exacte de l'appel signé) dans une ADR
  séparée 0053, avec la clé d'effet de l'ADR 0049 D4 dans le même train. Le registre
  `__gatedCalls` est abandonné. » Also bound by the product's ADR 0068 Revision 10 (2026-10-10,
  « un refus de porte termine le run »).
- Relates to: [0025](./0025-unified-agent-middleware-api.md) (the gate, intrinsic to the stack),
  [0032](./0032-secrets-redaction-and-no-log.md) (`noLog`),
  [0038](./0038-replay-as-evidence.md) (the replay journal),
  [0041](./0041-host-tools-on-catalog-runs-replayed-and-attested.md) (host tools journaled),
  [0044](./0044-cooperative-run-cancellation.md) (terminal `cancelled`),
  [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first, both SDKs in one release),
  [0049](./0049-the-host-keeps-each-checkpoint.md) (D3 a running checkpoint re-runs its node;
  **D4 the effect key, revised here**), [0051](./0051-the-signer-sees-and-signs-the-call.md)
  (`callKey`, `approvalScope: "call"`, D6 replaced by this ADR),
  [0052](./0052-a-context-budget-keeps-the-request.md) (a journal mark, absence = the old
  behaviour).

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
   D5's spent set lives in memory for one execution (branch `react.rs:309`, `:601`), so it cannot
   help across a resume.
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
| `anchorVersion` | `state.version` when this execution of the node began its loop (kept across every resume of that execution) |
| `iteration` | the loop index reached; `max_iterations` is a budget of the whole execution, not of a segment |
| `callSeq` | how many tool calls this execution has handed to `execute_tool_call` so far |
| `conversation` | the messages the next LLM request would carry: the seed as built and trimmed (ADR 0052), every assistant turn, every observation |
| `pending` | the calls of the stopped turn still to handle, in order — the gated one first, then the ones after it — each `{ id?, name, callInput, callKey, effectKey, callSeq }` |
| `trace`, `todos`, `memoryWrites` | what the final `AgentResult` must still report |
| `grantedCalls` | the gated calls this execution already executed on a grant (D2) |

`callInput` is the canonical JSON string ADR 0051 (as revised) hashes into `callKey`. The pending
call is executed from it, never from a re-serialized value.

**On the wire it is one opaque string.** `__agentResume[nodeId] = { "v": 1, "agent":
"<agentDigest>", "body": "<canonical JSON of the table above>", "sha256": "<hex of body>" }`. The
host keeps the checkpoint (ADR 0049) and may store it as `jsonb` and read it back with
`JSON.parse`, which turns `40.0` into `40` and rounds an integer above 2^53 (architect R3). A string
crosses that unchanged, so the restored conversation — and the pending input — are byte for byte
what the model wrote. `agent` is sha256 of the agent's fixed shape: name, provider, model, system
prompt, `max_iterations`, `approvalScope`, and each tool's name, `requires_approval`,
`content_scoped` and conditions. It is not a digest of the whole spec, so an engine upgrade that
adds a defaulted spec field does not invalidate it.

**Size.** The body is at most the agent's next LLM request plus the pending calls, so it is bounded
by the model's context window (and by the context budget, ADR 0052, when one is set). A run in
record mode already journals every such request; a suspended agent adds one more to its checkpoint.
A hard cap guards the host: `AGENT_RESUME_MAX_BYTES`, 4 MiB by default. Over it, the node fails
(`FailureCategory::Permanent`, `agent_resume_too_large: <bytes>`) instead of suspending: fail
closed, because a suspension that cannot resume exactly is the double effect this ADR removes.

**Who sees it.** No node does, except its owner. The runtime removes `__agentResume` from the state
it hands to every handler, as it adds `__injected` only to the handler's copy (`with_injected`,
`runtime.rs:375-386`); the resumed node finds its own continuation under the per-execution key
`__resume`, never persisted. Without this, the seed State of every later agent would carry other
agents' transcripts. It is in no event: the interrupt path emits no `NodeCompleted`. It is as
sensitive as the most sensitive channel the seed showed and the tool results it holds: a host
treats it as a `noLog` channel (ADR 0032) wherever it exports or shows state.

**When it goes.** The runtime clears `__agentResume[nodeId]` in the state mutation of the node's
completion (`runtime.rs:1033-1045`) and of its error routing (`:941`). A run that ends elsewhere
(cancelled while suspended, refused) keeps it, inert, in its terminal checkpoint; the host's
retention applies (ADR 0049, « Not decided »).

### D2 — On resume, the agent continues at the pending call and executes exactly that call

When the node runs and finds `__resume`, it does not build a seed, does not run `before_run`, and
does not call the model. It checks, then continues:

1. **Checks, fail closed.** `v` is known; `sha256(body)` matches; `agent` equals the digest of the
   agent the graph now declares; each pending call's `callKey` and `effectKey` recompute from its
   `callInput` and the anchor (D3). Any failure fails the node (`Permanent`,
   `agent_resume_refused: <which check>`), so `NodeFailed`, then `RunFailed` or the node's error
   edge. Before the resume, `resume_problems` reports the same reasons, so a host refuses it with a
   message rather than a failed run.
2. **The pending calls, in order.** Each goes through `before_tool` — the built-in gate and the
   installed middleware — against the `__approvedTools` the resume wrote, with `ToolCallCtx` now
   carrying the call's `effectKey`. A gated call may not have its input rewritten by a middleware
   (architect R2, `ToolControl::Deny`): the gate decides on the input that runs.
   - **Granted:** the handler runs with the parsed `callInput`; its observation is appended under
     the call's `id`, as `execute_tool_call` does (`react.rs:604-639`). It is recorded in
     `grantedCalls` (`{ callKey, effectKey, grant, resumed: true }`).
   - **Not granted, not refused** (a resume without the decision): the node suspends again with the
     same request and a byte-identical continuation. No LLM call is made. The suspension key is the
     same (D3), so the host files nothing new and `resume_problems` says « still pending ».
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
the whole execution. `usage` and `webSearch` stay the cost of *this segment's* LLM calls, as today
(each resume used to rebuild them from zero): a host that bills each segment counts each call once.
`todos` and `memoryWrites` are those of the whole execution; a memory write is keyed, so a host that
drained the suspension's list supersedes the same keys.

**An explicit restart.** A resume may pass `agentResume: "restart"` (TS
`ResumeCatalogGraphOptions.agentResume`, Python `agent_resume=`, the C-API spec): the bridge drops
the node's continuation and the agent starts from its first LLM call, the 2.6 behaviour. It is the
way out when the checks of step 1 refuse a continuation (the agent was changed), and the
operator's switch if the continuation misbehaves. It is recorded in the journal (D5).

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
execution began, carried by the continuation, so a call keeps its key across a suspension, a resume
and a crash re-run. It is an array, like the host node's key (`node_journal.rs:55-58`), so no field
can collide with another by containing a separator.

This replaces ADR 0049 D4's formula before it ships: the same goal, plus `callSeq` (two identical
calls of one execution get two keys) and the anchor (a resumed call keeps the key it was filed
under).

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

- `resume_problems` treats a rejected `tool:` request like a rejected `gate:` request: « request
  <id> (tool:refund) was rejected by <who> » (`catalog_approvals.rs:447-450` and the module docs at
  `:16`, `:33-37`). Every resume entry that checks approvals (TS, Python, C-API) then refuses.
- The host ends the run its way. The product does it (ADR 0068 Revision 10: `cancelled` checkpoint,
  `run_cancelled` with `reason: refused-at-tool-gate`). The engine vocabulary does not change: no
  `rejected` status, no `run_rejected` event.
- No « rejected » observation is fed to the model, and the continuation is never resumed after a
  refusal: nothing runs after it, no LLM call.

### D5 — Replay and verification re-derive it

**A journal mark, absence is the old behaviour** (ADR 0052 D2). `ReplayJournalWire`
(`lib.rs:153-169`) gains `agentResume: "continue" | "restart"`. A recording run writes `"continue"`,
or `"restart"` for a resume that passed it. A journal without the field (recorded before 0053)
replays as `"restart"`: a resumed agent starts from its first LLM call, as it did. An unknown value
fails the replay (`invalid replay_journal JSON`).

**Segment by segment.** A suspended-then-resumed run is replayed one segment at a time, each from
its own starting checkpoint with its own journal (ADR 0052, Consequences).

- The segment that suspends re-derives the same continuation: it is a pure function of the
  journaled LLM responses, the journaled tool results and the state at the node's entry.
- The segment that resumes restores the continuation from its starting checkpoint. The pending host
  call is served from the tool journal by `(name, inputHash)` (`lib.rs:296-350`), and its recorded
  `effectKey`, when the journal has one, must equal the re-derived one, or the replay fails
  (`tool_effect_mismatch`). The next LLM request matches the journaled one, since the conversation
  is restored byte for byte.

**Signed is executed.** `run_insight` gains `verify_granted_calls(attested, granted)`: every
approved attested record that carries a `callKey` must match, in order, a `grantedCalls` entry of
the replay with the same `callKey` (and the same `effectKey` when both carry one), and every
`grantedCalls` entry with a per-call grant must match an approved record. A mismatch reads « signed
X, executed Y ». `@ailu-ai/verify` runs it next to `verifyReplayDecisions`, taking the attested side
from `capsule.attestation.records` (architect R8, `packages/verify/src/verify.ts:116-117`); Python
exposes it through the native call. The attestation view may carry `effectKey` behind the same
opt-in as `callKey` (ADR 0051 D2): a record without it is unchanged byte for byte.

### D6 — `mapAgents`: the same, per spawn (decided here, shipped after the agent node)

The map node's entry holds one continuation per suspended spawn and the result of each finished
one: `{ "v": 1, "spawns": { "<index>": <continuation> | { "done": <result> } } }`. On resume only
the suspended spawns continue; a finished spawn's result is reused, not re-run (`node.rs:129-131`
becomes false). Spawn events stay as ADR 0050 defines them. Until it ships,
`approvalScope: "call"` on a `mapAgents` sub-agent is refused at compile with a typed error (as
architect R10 asks for the TS agent), and a map node resumes as today.

## How the runtime invariants hold

| Invariant | Holds because |
|---|---|
| Deterministic by default | The continuation is a pure function of recorded inputs; restore is a parse; `effectKey` and `callKey` are pure functions of recorded data. A replay knows from the journal mark whether to continue (D5). A live resume makes fewer LLM calls, never different ones for steps already answered. |
| Checkpoint after every node completion and state mutation | The continuation is written in the interrupt's mutation (one checkpoint, as today) and cleared in the completion's (one checkpoint, as today). No new mutation, no checkpoint inside the loop. |
| An event per lifecycle transition | Suspension: `RunSuspended`. Resume: `RunResumed` (`runtime.rs:682`) then `NodeStarted` (`:865`). Completion: `NodeCompleted`, whose `agentResult` carries `grantedCalls` (masked by `noLog` as before). A refused restore: `NodeFailed`. No transition is added, so no event is added. |
| Gates suspend cleanly and resume from the latest checkpoint | The suspension is unchanged. A resume loads the latest checkpoint (`runtime.rs:642-646`), the continuation is in it. A resume without a decision suspends again on the same request, without an LLM call. |
| No self-approval | Unchanged: the pending call passes the same gate against the grants the bridge validated (`lib.rs:512-518`). A continuation cannot grant anything. |
| Tenant isolation (rule 120) | The continuation lives in the run's checkpoint. `effectKey` contains the run id; the product still indexes it with `tenant_id`. |

## Compatibility

- **Old checkpoints.** A checkpoint suspended before 0053 has no `__agentResume`: the agent starts
  from its first LLM call, as in 2.6. `approvalScope: "call"` is not adopted before 0053 ships
  (owner's decision 3), so no such run is waiting on a per-call grant.
- **Old journals** replay as recorded (D5).
- **Downgrade.** An engine older than 0053 sees `__agentResume` as an ordinary channel and would put
  it in an agent's seed State. A host that rolls back drops that channel from waiting runs first.
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
    `AGENT_RESUME_KEY`. Rust `agents-core`: `AgentContinuation`, `ReActAgent::run_resumable` /
    `ReActAgent::resume_from`, `ApprovalRequestItem.effect_key`, `AgentResult.granted_calls`,
    `ToolContext`, `InMemoryToolRegistry::register_with_context`, `effect_key_of`;
    `ToolCallCtx.effect_key` (a new public field: code that builds a `ToolCallCtx` by hand must add
    it). Bridge: `ApprovedTool.effect_key`, `ToolResultWire.effect_key`, the journal's
    `agentResume`, the resume option, `verify_granted_calls`.
  - TypeScript: `ResumeCatalogGraphOptions.agentResume`, the tool context argument, `effectKey` on
    pending approvals, `AgentResult.grantedCalls`, `verifyGrantedCalls`, `effectKeyOf` (napi, one
    implementation, like ADR 0051's `callKeyOf`).
  - Python: `agent_resume=`, `HostTool` / `HostToolInput`, `verify_granted_calls`, `effect_key_of`.
- **Behaviour change.** A resumed agent makes fewer LLM calls and repeats no call. Its iteration
  budget covers the whole execution: an agent that used to get a fresh budget at each resume no
  longer does. Both go in the changelog's « Behaviour » section.
- **Versioning.** A minor. Proposed: **2.7.0** ships the R1 fix and ADR 0051 D1–D3 as revised;
  **2.8.0** ships this ADR, ADR 0049 D4 as revised here, and ADR 0051 D4/D5 together (D4 is only
  safe with this ADR, architect R9).

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
   node narrowing an engine-owned channel.
7. **On a refusal, tell the model and continue.** Contradicts ADR 0068 Revision 10: nothing runs
   after a refusal.

## Consequences

- **What is signed is what runs**, under both scopes, for the call that opened the request; under
  `"call"`, for every gated call. `verify_granted_calls` makes it checkable by a third party.
- **Cost.** One LLM call fewer per gated call per resume, and no repeated tool calls. One string the
  size of one LLM request per suspended agent in its checkpoint.
- **Data protection.** The checkpoint of a suspended agent holds its conversation: the seed (State,
  `noLog` channels included, as the prompt did), tool results, model output. It was already in the
  model journal of a recorded run. A host masks it like a `noLog` channel in exports; a proof
  capsule that must replay a resume segment carries it, so the capsule's retention and access rules
  apply.
- **Host work.** To get at-most-once effects, a host tool keys its effect on `effectKey`. To bind a
  signature to one occurrence, the host stores the request's `effectKey` and gives it back with the
  grant.
- **Failure modes made loud.** A continuation over the cap, or that does not match the agent, fails
  the node instead of resuming differently; the host can restart the agent on purpose.

## What the product changes to consume it

1. Bump to 2.8.0. Treat `__agentResume` as `noLog` in checkpoint reads, exports and capsules.
2. Store `effectKey` with each approval request (it is in `subject`); give it back in
   `approvedTools` on resume. Index by `(tenant_id, run_id, effect_key)`, never by the key alone.
3. Key connector writes (refund, payout) on the tool context's `effectKey`.
4. Then set `approvalScope: "call"` on the agents whose gated tools move money or data.
5. Show « 1 signature, 1 appel » from `grantedCalls`; run `verifyGrantedCalls` in verify-replay.

## Implementation plan (small batches, ADR 0048 DORA)

One concern per branch, each green on its own (`cargo test` of the crate, the SDK's typecheck, lint
and tests, the golden and `*.rust.test.ts` suites it touches) and revertible alone. In this order:

1. **R1 fix** — `suspension_key` reads `callKey`; « all decided and suspended again → file again »;
   catalog-path tests A then B, A then A, two conditioned calls (also fixes ADR 0046). Before #328.
2. **#327 revisions** — ADR 0051 D1–D3 with R2–R8, `callInput`, `callKeyOf`, the effective grant in
   the signed view. Cut **2.7.0** before #328 merges.
3. **#328** rebased on 1, merged into `main`, not published (owner's decision 3), with R9's
   documenting test and R10's typed refusal.
4. **0053-a, runtime seam** (`graph-runtime` only): `NodeOutput.continuation`, `__agentResume`
   engine-owned, written at the interrupt, cleared at completion and error routing, hidden from
   handler views, `__resume` for its owner. No node uses it yet: no behaviour change.
5. **0053-b, the effect key** (Rust): `effect_key_of`, `anchorVersion` and `callSeq` in the loop,
   `effectKey` on requests, host-tool payloads and the tool journal, the `effectKey` tier of
   `suspension_key`. Amends ADR 0049's status line (D4 revised by 0053).
6. **0053-c, the continuation** (agents-core + bridge): `AgentContinuation`, the restore checks, the
   cap, the node handler, the journal mark, `agentResume: "restart"`. Tests: A, B, C gated in
   sequence (each executed once, three requests, model called once per turn); A then A again; two
   gated calls in one turn; a resume without a grant suspends again identically with zero LLM
   calls; a pre-0053 checkpoint restarts; a crash after the resume checkpoint redelivers the same
   `effectKey`; replay with and without the mark; a changed agent is refused, then restarted.
7. **0053-d, bound grants**: `ApprovedTool.effectKey`, `<callKey>@<effectKey>` in `__approvedTools`,
   `resume_problems` matching.
8. **0053-e, refusal** (D4): `resume_problems` blocks on a rejected tool request. Independent: it
   may move earlier if the owner wants it in 2.7.0.
9. **0053-f, SDK surfaces**: TS and Python options, tool context, `HostTool`, `effectKeyOf`, types,
   parity tests in both SDKs and the C-API.
10. **0053-g, verification**: `verify_granted_calls`, `@ailu-ai/verify` (with R8), the Python
    docstring, `effectKey` in the attestation view behind the opt-in.
11. **Release 2.8.0**: publishes 0053, ADR 0049 D4 and ADR 0051 D4/D5. Then the product (above).
12. **0053-h, `mapAgents`** (D6), in a later minor.

## Not decided here

- Checkpointing inside the loop (alternative 4), or any durability tier.
- An engine entry that ends a refused run itself (the product's `RunRefusal` does it today).
- Showing the continuation to a signer (« the agent's reasoning before this call »): product UX.

## Open questions for the owner

1. **The design** — continuation handed over by the node, written by the runtime in `__agentResume`,
   one opaque string per node. *Recommended: yes.*
2. **ADR 0049 D4 revised** — the effect key gains `callSeq` and the anchor version, and becomes the
   occurrence identity of filing, grants and proofs. *Recommended: yes, before it ships.*
3. **Over the cap** — fail the node (fail closed) or suspend without a continuation (the old
   re-run)? *Recommended: fail, cap 4 MiB, measured on the beta before 2.8.0.*
4. **A changed agent** — refuse the continuation and let the host pass `agentResume: "restart"`?
   *Recommended: yes; it is also the operator's switch, so no global flag.*
5. **Bound grants** — give the signature's occurrence back (`effectKey` on `ApprovedTool`) rather
   than have the runtime remove spent grants from `__approvedTools`? *Recommended: bound grants.*
6. **`usage` per segment** (as today) rather than for the whole execution? *Recommended: per
   segment, so a host never bills a call twice.*
7. **`mapAgents`** later, with `"call"` refused on its sub-agents until then? *Recommended: yes.*
