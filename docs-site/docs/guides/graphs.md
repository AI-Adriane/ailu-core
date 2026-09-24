---
title: "Graphs"
description: "Channels hold the run's state, nodes change it, edges order the steps. Routing, reducers, retries and error branches."
---

# Graphs

A graph has three parts:

- **Channels**: the run's state. Each has a name, a type label and a default value.
- **Nodes**: the steps. A node reads the state and returns the channels it changes.
- **Edges**: the order of the steps.

```ts file=packages/graph-sdk/examples/docs/graphs-basics.ts region=example
```

`createGraph` returns a builder. `compile()` checks the graph (unknown nodes, missing handlers,
dangling edges) and returns a `CompiledGraph` you can `run()` many times. Each run gets its own
state and its own `runId`.

The first node you add is the entry. To start elsewhere, call `.entry("nodeId")`.

## Read and write state

A node handler receives `(input, state)`. Read channels from `state.channels`; they are typed
from your `.channel()` declarations. Return an object with the channels to update. Return `{}`
to change nothing.

`run(data)` seeds channels from `data`, and every channel you don't pass starts at its default.
`run()` resolves with the final state: `status`, `runId`, `currentNodeId` and `channels`.

## Route with conditions

A conditional edge is taken when its predicate returns `true`. The predicate has a name, so the
graph stays data you can inspect, store and render; the function stays in your code.

```ts file=packages/graph-sdk/examples/docs/graphs-routing.ts region=example
```

When a node finishes, the engine looks at its outgoing edges in the order you added them and
follows the first one that applies: a plain edge, or a conditional edge whose predicate returns
`true`. If none applies, the run ends there. A predicate that throws fails the run rather than
silently taking another branch.

## Combine updates with reducers

By default a node's value **replaces** the channel's value. A reducer changes that:

| Reducer | Effect |
| --- | --- |
| `replace` (default) | The new value replaces the old one. |
| `append` | The value is added to the end of the list; an array adds each of its items. Useful for logs and messages. |
| `merge` | The object's keys are written over the existing object, one level deep. |

```ts file=packages/graph-sdk/examples/docs/graphs-reducers.ts region=example
```

## Retry, then branch on failure

A node can retry before it fails the run. An error edge sends the run somewhere else once the
retries are spent, instead of failing it.

```ts file=packages/graph-sdk/examples/docs/graphs-errors.ts region=example
```

A thrown error that is not caught by an error edge fails the run: `status` is `"failed"` and a
`run_failed` event says why.

## Keep a channel out of logs

Mark a channel `noLog: true` and its value is masked in every run event and log. It is still
saved in checkpoints, so the run can resume. See [Governance](./governance.md#keep-secrets-out-of-logs).

## Next

- Add an LLM step: [Agents and models](./agents.md).
- Stop for a person: [Human approval](./human-approval.md).
- Run steps in parallel: [Multi-agent](./multi-agent.md).
