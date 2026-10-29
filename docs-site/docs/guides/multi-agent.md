---
title: "Multi-agent"
description: "Run agents in parallel, one per item, as isolated sub-agents, as reusable subgraphs, or as a council that ranks its own answers."
---

# Multi-agent

Five building blocks cover most multi-agent designs. Pick the one that matches the shape of
your problem.

| You have... | Use |
| --- | --- |
| A fixed set of steps that can run at the same time | [`fanOut`](#run-steps-in-parallel) |
| A list, and one agent per item | [`mapAgents`](#one-agent-per-item) |
| A task to hand to an isolated sub-agent that reports back | [`taskNode`](#delegate-to-a-sub-agent) |
| A graph you want to reuse as one step | [`subgraph`](#reuse-a-graph-as-a-node) |
| A question worth several opinions | [`council`](#ask-a-council) |

## Run steps in parallel

`fanOut(from, [branches], joinAt)` runs the branches at the same time, each from the same state,
then continues at `joinAt` once all of them are done. Give each agent its own `outputChannel` so
their results don't overwrite each other.

```ts file=packages/graph-sdk/examples/docs/multi-fan-out.ts region=example
```

Branch updates are applied in the order you list the branches, whichever finishes first, so the
result is the same on every run.

## One agent per item

`mapAgents` runs one sub-agent per item of a list channel, in parallel, and collects the results
in input order.

```ts file=packages/graph-sdk/examples/docs/multi-map-agents.ts region=example
```

With `suspendForApproval: true`, a sub-agent that wants a gated tool suspends the whole run, as a
single agent would.

### Host fan-out budgets

Both `mapAgents` and catalog `mapSubgraph` default to **128 items per array**, **8 concurrent
futures per map**, and **512 admitted mapped items per start/resume/replay traversal**. Dynamic
nesting is fixed at 8. An oversized array fails before its children start; it is never truncated.

Set trusted host options at compilation or on each catalog call:

```ts
const limits = { maxItems: 256, maxConcurrency: 4, maxTotalItems: 1024 };
const app = builder.compile({ fanOutLimits: limits });
// Catalog start, resume and replay accept the same host option:
const outcome = await runCatalogGraph(definition, { fanOutLimits: limits });
```

Python uses `builder.compile(fan_out_limits={"maxItems": 256})`,
`GraphRunner(spec, fan_out_limits=...)`, or `fan_out_limits` on each catalog helper. Rust uses
`FanOutLimits::new(items, concurrency, total)?` and `runtime.with_fan_out_limits(limits)`.

Values must be positive integers within the hard ceilings: **1,024 items**, **32 concurrent
futures**, and **4,096 total items**. Invalid settings raise
[`InvalidFanOutLimitsError`](../reference/errors.md#ailu_invalid_fan_out_limits) at construction.
Graph definitions, metadata, channels and saved state cannot raise their own budgets.

Concurrent children receive deterministic shares of the remaining allowance. A child can fail
below the total limit if its share is too small; siblings cannot borrow while their batch runs.
Unused shares return afterward. For larger workloads, configure reviewed host limits, flatten
or batch nested maps, or schedule separate bounded runs. Sequential maps in one traversal still
share its total allowance. Supply host options again on catalog resume/replay: settings and
remaining allowance are not checkpointed. These are work bounds; provider-call, token, byte,
time, cross-run and monetary quotas remain host responsibilities. Nested maps may exceed the
per-map concurrency count in aggregate.

## Delegate to a sub-agent

`taskNode` runs a sub-agent in its own child graph. It sees only the `objective` channel, and
only its `report` comes back: the parent never sees its intermediate work, and it never sees the
parent's other channels.

```ts file=packages/graph-sdk/examples/docs/multi-task-node.ts region=example
```

Rename the channels with `objectiveChannel` and `reportChannel`. By default the report is kept
short (`compress: true`); set `compress: false` for the full answer.

## Reuse a graph as a node

`subgraph` embeds another builder as one node. The mappings list which channels cross the
boundary, written `destination: source`.

```ts file=packages/graph-sdk/examples/docs/multi-subgraph.ts region=example
```

The child shares the parent's run: a human gate inside it suspends the parent, and resuming the
parent continues the child.

## Ask a council

`council()` builds a complete graph: members answer in parallel, reviewers rank the answers
without knowing who wrote them, and a chair writes the final answer from the ranking. It runs
with `runCatalogGraph`.

```ts file=packages/graph-sdk/examples/docs/multi-council.ts region=example
```

Each seat sees only what it needs: members see the question; reviewers see the question and the
anonymized answers; the chair sees the answers and their ranking. The `fieldKey` channel maps
each label back to its member for your audit trail.

## Next

- An agent that plans and delegates on its own: [Deep agents](./deep-agents.md).
- A complete example: [Parallel agents](../examples/parallel-agents.md).
