---
sidebar_position: 16
title: Parallel fan-out over items (mapAgents)
description: Run the SAME agent concurrently over every item in a channel array, then join the per-item results in input order — the Parallel pattern.
tags: ["agents", "orchestration"]
difficulty: intermediate
---

# Parallel fan-out over items (mapAgents)

`mapAgents` (ADR 0027 phase 4b) fans **one** sub-agent out over **every item in an array
channel**, concurrently, and joins the per-item results back into an array — deterministic input
order, not completion order. Use it when you have N interchangeable items and want the same
agent's judgment applied to each one, in parallel, instead of one agent processing them serially.

Distinct from [`fanOut`](/docs/recipes/hierarchical-delegation): `mapAgents` runs the **same**
sub-agent config over N **items**; `fanOut` dispatches to N **different**, heterogeneous
specialist nodes.

## 1. `mapAgents` over a seeded array

`mapAgents` auto-declares its own `overChannel`/`joinAt` channels (both `json`, default `[]`) — no
`.channel(...)` call needed for them.

```ts
import { createGraph, DefaultLLMGateway } from "@ailu/graph-sdk";

const graph = createGraph({ name: "comprehensive-research" })
  .mapAgents("researcher", {
    overChannel: "researchAngles",   // the array to fan out over
    joinAt: "researchFindings",      // per-item results land here, input order
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: {
        system:
          "Research the given angle thoroughly. Ground every claim in cited sources."
      }
    }
  })
```

## 2. Synthesize the joined findings

```ts
  .agentNode("synthesize", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Synthesize the joined research findings into one report." },
    outputChannel: "report"
  })
  .edge("researcher", "synthesize")
  .compile();

const result = await graph.run({
  researchAngles: [
    { angle: "domain", brief: "What's already known, key concepts, prior art." },
    { angle: "market", brief: "Competitors, demand signals, positioning." },
    { angle: "technical", brief: "Viable approaches, trade-offs, risks." }
  ]
});
```

Each of the 3 angles runs the **same** `researcher` sub-agent concurrently; `researchFindings`
collects all 3 results in the order the angles were listed (not the order they finished), so
`synthesize` always sees a stable, deterministic array to work from.

## Governance still applies per spawn

Each `mapAgents` spawn is a full sub-agent — skills/memory/fs/planning all apply exactly as they
would on a plain `agentNode`, and a spawn that needs approval suspends the **whole** map (unless
`suspendForApproval: false` is set on the `mapAgents` config itself, in which case only that one
spawn suspends while the others continue). Checkpointing and replay cover every spawn.

## See also

- [Hierarchical delegation (fanOut)](/docs/recipes/hierarchical-delegation) — the heterogeneous-specialists counterpart.
- [React planner + critic](/docs/recipes/react-planner-critic) — a single-agent alternative when the items aren't independent.
