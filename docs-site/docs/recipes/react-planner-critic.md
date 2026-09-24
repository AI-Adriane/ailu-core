---
sidebar_position: 7
title: ReAct agent with planner & critic
description: A togglable planner → agent ⇄ tools → critic → finalize graph, and how it compares to the same thing in LangGraph.
tags: ["agents"]
difficulty: intermediate
---

# ReAct agent with planner & critic

A common deep-agent shape: an optional **planner** drafts an approach, an **agent** reasons and
calls tools in a loop, an optional **critic** reviews and either approves or sends it back for
refinement, then **finalize** writes the answer.

```text
START → [planner] → agent ⇄ tools → [critic] → finalize → END
                          ↑__________|   (critic → agent refinement loop)
```

In Ailu the **agent ⇄ tools loop is built into `agentNode`** — you pass the tools and the
ReAct loop (reason → call tool → observe → repeat) runs inside the node. There is no separate
tool node to wire, and no "route back to the agent after tools" edge to manage.

```ts
import { createGraph, type LLMGateway, type ToolRegistry } from "@ailu-ai/graph-sdk";

export function buildReActGraph(deps: {
  llm: LLMGateway;
  tools: ToolRegistry;
  systemPrompt: string;
  plannerPrompt: string;
  criticPrompt: string;
  finalPrompt: string;
  maxIterations: number;
  enablePlanner: boolean;
  enableCritic: boolean;
}) {
  const {
    llm, tools, systemPrompt, plannerPrompt, criticPrompt, finalPrompt,
    maxIterations, enablePlanner, enableCritic
  } = deps;

  let g = createGraph({ name: "react" })
    .channel("approved", { type: "boolean", default: false })
    .channel("iterations", { type: "number", default: 0 });

  if (enablePlanner) {
    g = g.agentNode("planner", {
      llm, tools,
      prompt: { system: `${systemPrompt}\n${plannerPrompt}` },
      outputChannel: "plan"
    });
  }

  // The agent ⇄ tools loop is internal to agentNode — pass `tools` and it runs the ReAct loop.
  g = g.agentNode("agent", { llm, tools, prompt: { system: systemPrompt }, maxIterations });

  if (enableCritic) {
    g = g.agentNode("critic", {
      llm,
      prompt: { system: `${systemPrompt}\n${criticPrompt}` },
      outputChannel: "critique"
    });
  }

  g = g.agentNode("finalize", {
    llm,
    prompt: { system: `${systemPrompt}\n${finalPrompt}` },
    outputChannel: "answer"
  });

  if (enablePlanner) g.edge("planner", "agent");
  else g.entry("agent");

  if (enableCritic) {
    // After the agent, the critic reviews; approve (or budget spent) → finalize, else refine.
    g.edge("agent", "critic")
      .conditionalEdge(
        "critic", "agent", "needsRefinement",
        (s) => !s.channels.approved && s.channels.iterations < maxIterations
      )
      .conditionalEdge(
        "critic", "finalize", "approved",
        (s) => s.channels.approved || s.channels.iterations >= maxIterations
      );
  } else {
    g.edge("agent", "finalize");
  }

  return g.compile();
}
```

The conditions are **named predicates** (`needsRefinement`, `approved`) registered with the
graph — inspectable and never `eval`'d. The channels are typed: `s.channels.approved` is a
`boolean`, `s.channels.iterations` a `number`, with no manual state class or `any` cast.

## Even simpler: reflection as middleware

If the critic is just "score the answer; if weak, flag it", you don't need a critic node at all —
add the **reflection** efficiency middleware (or the `frontier-careful` / `governed-deep`
[profile](/docs/advanced-agents/middleware-and-profiles), which include it):

```ts
g.agentNode("agent", {
  llm, tools,
  prompt: { system: systemPrompt },
  middleware: [{ kind: "reflection" }]   // one self-critique after the run; flags a weak answer
});
```

## How it compares to LangGraph

The same graph in LangGraph needs a manual `ToolNode`, a `routeAfterAgent` function, the
`tools → agent` edge, and a `StateGraph` typed `any` (its node-name generics don't survive
conditionally-added nodes). In Ailu:

| | LangGraph | Ailu |
| --- | --- | --- |
| agent ⇄ tools | manual `ToolNode` + `routeAfterAgent` + `tools → agent` edge | built into `agentNode({ tools })` |
| typing | `const workflow: any` | typed channels, no `any` |
| conditions | arbitrary functions | named, inspectable predicates (never `eval`'d) |
| governance | wire it yourself | the approval gate is intrinsic; sensitive tools gate automatically |
| durability | add a checkpointer | checkpointed + resumable by default |

## Next

- [Middleware & profiles](/docs/advanced-agents/middleware-and-profiles)
- [Multi-agent orchestration](/docs/building/multi-agent-orchestration)
- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react)
