# ADR 0050 — Each fan-out spawn reports its lifecycle

- Status: **Accepted** — approved by Mathieu 2026-10-07 on AI-Adriane/ailu#2034 (« il faut faire
  tout ce que tu dis, voire faire évoluer le moteur si besoin »).
- Date: 2026-10-07
- Deciders: Mathieu (owner)
- Driven by the product issues AI-Adriane/ailu#2034 (show each sub-agent of a fan-out under its
  node) and AI-Adriane/ailu#2033 (a list sent as text made a fan-out do nothing, silently).
- Relates to: [0027](./0027-async-subagents-and-streaming.md) (the `mapAgents` fan-out),
  [0033](./0033-token-streaming-and-subagent-tagging.md) (`token_delta`, its `spawnId`, and
  « durability ≠ observability »), [0038](./0038-replay-as-evidence.md) (the recorded clock),
  [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first, both SDKs in the same release),
  [0049](./0049-the-host-keeps-each-checkpoint.md) (a checkpoint per transition).

## Context

A `mapAgents` node runs one sub-agent (a *spawn*) per item of `overChannel`, concurrently, and
writes the results in input order into `joinAt`. The host sees `node_started` and
`node_completed` for the node and nothing in between: which item each spawn works on, when it
ends, what it cost, whether it failed or waits for an approval. Only `token_delta` carries a
`spawnId`, and only when token streaming is on. A host cannot show one line per sub-agent.

Separately, a present `overChannel` that is not a JSON array (a list typed into a form and sent as
the string `'["a","b"]'`) is read as no items: the node writes `[]` in a few milliseconds and the
next node works on nothing, with no error.

## Decision

### D1 — Four events per spawn

The `mapAgents` handler sends, for each item at index `i`:

| `type` | Fields (beyond `type`, `runId`, `nodeId`, `timestamp`) |
| --- | --- |
| `spawn_started` | `spawnId`, `itemIndex`, `item` |
| `spawn_completed` | `spawnId`, `itemIndex`, `output`, `usage?` |
| `spawn_failed` | `spawnId`, `itemIndex`, `error` |
| `spawn_suspended` | `spawnId`, `itemIndex`, `reason` |

- `spawnId` is `i`, the id that spawn's `token_delta` events already carry, so a host correlates
  the two. `itemIndex` is also `i`; it names the position in the list, and stays meaningful should
  spawn ids ever stop being indices.
- `runId` / `nodeId` are those of the run executing the node, the same as its `node_started`.
  (Inside a subgraph child this is the child run id; `token_delta` carries the top-level run id
  there. Correlate on `(nodeId, spawnId)` within a node execution.)
- `item` is the element of `overChannel`, sent as-is. Items can be large; the host truncates for
  display.
- `output` is the spawn's `AgentResult`, the value written at index `i` of `joinAt`.
- `usage` is the spawn's token usage summed over its LLM calls (`AgentResult.usage`:
  `promptTokens`, `completionTokens`, `cacheReadTokens?`, `cacheWriteTokens?`). The ReAct loop
  always fills it today; the field is optional so an unknown usage is omitted, never invented.
- `spawn_failed`: the spawn's agent returned an error (a gateway error). As before, the node writes
  `{ "error" }` at that index and goes on; the event only makes it visible.
- `spawn_suspended`: the spawn flagged `requiresHumanReview` and the node has
  `suspendForApproval`; `reason` is `agent-approval-required`, the run's `run_suspended` reason.
  Without `suspendForApproval`, such a spawn reports `spawn_completed`.

### D2 — Observational, not durable

The events go straight to the host's `on_event` callback (napi `onEvent`, Python `on_event`, the
C-API event callback), through a `SpawnEventSink` the bridge installs on the handler — the path
`token_delta` takes. They are **never put on the runtime `EventBus`**: they are absent from the
engine's events vector, from checkpoints, from the replay journal, and carry no `checkpointId`
(ADR 0049 D2: they write none). Their `timestamp` is the wall clock, not the runtime clock.

Why: a node handler only receives the state, not the bus; and putting them on the bus would make
concurrent spawns read the runtime clock in a non-deterministic order, which a record-mode run
captures into the clock sequence a replay re-feeds (ADR 0038). Observational events change none of
that. A host that wants them durable journals them from `on_event` — the product does.

Consequences that follow:

- **Additive.** The node's own `node_started` / `node_completed` (or `node_failed`,
  `run_suspended`) are emitted exactly as before; the checkpoint after the node is unchanged; the
  join is still by input index, so run state stays deterministic.
- **Replay** ignores them the way it ignores `token_delta`: a replayed `mapAgents` node sends them
  again to `on_event`, and nothing compares them.
- **Ordering.** Per spawn, `spawn_started` precedes its one terminal event. Spawns run
  concurrently, so events of different spawns interleave in any order. All of a node execution's
  spawn events fall between its `node_started` and its closing event.
- **Re-runs.** A node that runs again (resume after an approval, a `retryPolicy` attempt, a replay)
  re-runs every spawn and reports a full new set. A host keeps the latest per
  `(runId, nodeId, spawnId)`.

### D3 — `mapSubgraph` sends no spawn events

The runtime's `mapSubgraph` fan-out is out of scope for D1. Each of its children is a run of its
own that already emits durable lifecycle events (`node_*`, `run_*`) under the id
`<parentRunId>:<nodeId>:<index>`, so a host can already show one line per child. Adding spawn
events there would put them on the bus and read the runtime clock (D2), changing recorded clock
sequences for no new information.

### D4 — A fan-out over something that is not a list fails

For `mapAgents` **and** `mapSubgraph`:

- `overChannel` absent, `null` or `[]` → no spawns, an empty join: the deterministic no-op of
  before.
- `overChannel` present and not an array (string, object, number, boolean) → the node fails with
  `<kind> node '<nodeId>': overChannel '<channel>' must be a JSON array, got <type>`, e.g.
  `mapAgents node 'researcher': overChannel 'researchAngles' must be a JSON array, got string`.
  `mapAgents` returns a failure classified `permanent` (a retry cannot fix the input), so
  `retryPolicy` and error edges apply as for any node; `mapSubgraph` emits `node_failed`
  (`permanent`) and fails the run. No spawn event is sent.

This is a behaviour change: a graph that relied on a non-list being read as `[]` now fails. It is
the fix the product asked for; the engine does not coerce a JSON string into a list (the host owns
the meaning of its form fields).

One helper, `ailu_graph_runtime::fan_out_items`, reads the items for both fan-outs.

## Parity

Rust (`graph-runtime` `RunEvent`, `agents-core` `map_node_handler`, `runtime-bridge`), the napi
bridge and the TypeScript SDK (`RunEvent` union, `SpawnUsage`), the pyo3 bridge and the Python SDK
(event dicts with the same keys), and the C-API (same JSON payloads) ship in the same release.
