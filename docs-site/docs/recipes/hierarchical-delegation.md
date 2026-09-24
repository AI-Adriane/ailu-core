---
sidebar_position: 17
title: Hierarchical delegation (manager + specialists via fanOut)
description: A manager agent dispatches to N different, heterogeneous specialist nodes concurrently, then joins at a report node — the primitive council() itself is built on.
tags: ["agents", "orchestration"]
difficulty: intermediate
---

# Hierarchical delegation (manager + specialists via fanOut)

`fanOut` (ADR 0013) dispatches from one node to a **fixed set of different, heterogeneous
specialist nodes**, running them concurrently on the Rust engine from the same pre-fan-out state
snapshot, then joins at a report node once every branch completes — deterministic merge order
(`parallelTo` order), regardless of which branch finishes first. It's the primitive
[`council()`](/docs/recipes/governed-council-decision) itself is built on, exposed directly for a
"manager delegates to specialists" shape.

Distinct from [`mapAgents`](/docs/recipes/parallel-fan-out): `fanOut` dispatches to N **different**
nodes (a researcher, an analyst, a reviewer — each its own prompt/config); `mapAgents` runs the
**same** sub-agent config over N interchangeable **items**.

## 1. Build the manager + specialist nodes

```ts
import { createGraph, DefaultLLMGateway } from "@ailu/graph-sdk";

const llm = new DefaultLLMGateway();

const graph = createGraph({ name: "hierarchical-delivery" })
  .agentNode("manager", {
    llm,
    prompt: {
      system:
        "You are a delivery orchestrator. Decide which role acts next. Delegate; do not do " +
        "every role's job yourself."
    },
    outputChannel: "managerBrief"
  })
  .agentNode("researcher", {
    llm,
    prompt: { system: "Research the change's context and prior art." },
    outputChannel: "researchFindings"
  })
  .agentNode("analyst", {
    llm,
    prompt: { system: "Analyze feasibility and trade-offs." },
    outputChannel: "analysisFindings"
  })
  .agentNode("reviewer", {
    llm,
    prompt: { system: "Review the proposed approach for risk." },
    outputChannel: "reviewFindings"
  })
  .agentNode("technical-writer", {
    llm,
    prompt: { system: "Synthesize the specialists' findings into one final report." },
    outputChannel: "finalReport"
  })
```

## 2. Wire the fan-out

```ts
  // One edge into the first branch is enough for a valid entry-reachable DAG — fanOut's own
  // parallelTo/joinAt drive the rest at runtime (validateGraph has no reachability check beyond
  // the entry node existing). Same convention council() itself uses internally.
  .edge("manager", "researcher")
  .fanOut("manager", ["researcher", "analyst", "reviewer"], "technical-writer")
  .compile();

const result = await graph.run({ task: "Add rate limiting to the public API." });
```

`manager` runs first; then `researcher`/`analyst`/`reviewer` run **concurrently**, each from the
same post-manager state snapshot; `technical-writer` runs once all three complete, seeing all
three findings merged in the declared `parallelTo` order.

## Governance still applies per branch

Each fanned-out node is a full agent node — skills/memory/fs/planning, the intrinsic approval
gate, and checkpointing all apply exactly as on any other `agentNode`. A branch that needs
approval suspends the whole fan-out; resume continues from the same checkpoint.

## See also

- [Parallel fan-out (mapAgents)](/docs/recipes/parallel-fan-out) — the same-agent-over-N-items counterpart.
- [Council](/docs/recipes/governed-council-decision) — multi-seat deliberation built on this same `fanOut` primitive.
