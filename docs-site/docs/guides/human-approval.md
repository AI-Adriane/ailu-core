---
title: "Human approval"
description: "Stop a run at a human gate, show what it waits for, and resume it once a person approves."
---

# Human approval

A human gate is a node that stops the run. Everything before it has run and is checkpointed;
nothing after it runs until you resume.

```ts file=packages/graph-sdk/examples/docs/human-gate.ts region=example
```

## The approval flow in an app

1. **Run.** `run()` returns as soon as the run reaches the gate, with `status: "suspended"` and
   `currentNodeId` set to the gate. Save the `runId`.
2. **Show.** Display what needs review. The channels are on the returned state;
   `app.explain(runId)` describes the wait in words.
3. **Resume.** When a person approves, call `app.resume(runId)`. The run continues from the gate.

`resume` works on the `CompiledGraph` instance that started the run, while your process is up.
To resume after a restart or in another process, run the graph with `runCatalogGraph` and keep
its state: see [Long-running runs](./long-running.md).

## Gate a tool, or gate the run?

| You want to... | Use |
| --- | --- |
| Review a step's output before the run goes on (a draft, a plan, a price) | `humanGate` |
| Approve a specific action an agent wants to take (a refund, a deletion) | a tool with `requiresApproval`. See [Tools and approval](./tools.md). |

## Several gates

A graph can have as many gates as it needs. Each `resume` runs until the next gate or the end.
A gate inside a [subgraph](./multi-agent.md#reuse-a-graph-as-a-node) suspends the parent run
too, and `resume` on the parent continues the child.

## Next

- Wait for a webhook or a date instead of a person: [Long-running runs](./long-running.md).
- Sign each decision: [Governance](./governance.md).
