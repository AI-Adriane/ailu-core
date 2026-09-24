---
sidebar_position: 2
title: Build a governed deep agent (end to end)
description: A progressive walkthrough — lead agent with planning, governed filesystem, sub-agents via taskNode, skills overlay, approval gates, and live observation.
tags: ["agents", "governance"]
difficulty: advanced
---

# Build a governed deep agent (end to end)

This recipe stitches all the pieces of a governed deep agent together in one **progressive tutorial**: a lead agent that plans with `writeTodos`, keeps a scratchpad on the governed filesystem, spawns isolated sub-agents with `taskNode`, loads playbooks with `skills`, gates a sensitive tool via `suspendForApproval`, and observes the whole run live.

Each step adds one capability on top of the previous. At the end you have a complete, checkpointed, audited, multi-agent loop — the **builder persona's missing spine**.

## Step 1 — the lead agent with writeTodos planning

Start with a lead agent that breaks its goal into a durable plan.

```ts
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  writeTodosTool,
  TODOS_CHANNEL,
  MockLLMProviderAdapter,
  type LLMGateway
} from "@ailu-ai/graph-sdk";

const mockLLM = (): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new MockLLMProviderAdapter({
      provider: "anthropic",
      response: {
        content:
          "I'll break this down. First, I need to research the topic, then delegate writing to ensure quality.",
        toolCalls: [
          {
            id: "t1",
            name: "writeTodos",
            input: {
              todos: [
                { id: 1, text: "Research the topic and gather facts", status: "pending" },
                { id: 2, text: "Write the article draft", status: "pending" },
                { id: 3, text: "Review and finalize", status: "pending" }
              ]
            }
          }
        ],
        stopReason: "tool_use",
        usage: { promptTokens: 0, completionTokens: 0 },
        model: "mock",
        provider: "anthropic"
      }
    })
  );
  return gateway;
};

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .agentNode("lead", {
    llm: mockLLM(),
    prompt: {
      system: "You are a planning agent. Break the goal into a clear todo list with writeTodos."
    },
    tools,
    todosChannel: TODOS_CHANNEL
  })
  .compile();

const result = await app.run();
console.log("Todos written:", result.channels.todos);
```

**What it adds:** The `writeTodos` tool writes a durable checklist (`[{ id, text, status }]`) to `TODOS_CHANNEL`. The list survives suspension and downstream nodes read it.

## Step 2 — add the governed filesystem for scratchpad

Give the lead agent a virtual scratchpad bounded by a fail-closed path policy.

```ts
import { createGraph, DefaultLLMGateway, InMemoryToolRegistry, writeTodosTool, TODOS_CHANNEL } from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .fsPolicy([
    { glob: "work/**", verb: "write" },      // write freely under work/
    { glob: "notes/**", verb: "read" }       // read-only for notes
  ])
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: "Plan the task. Write todos, then sketch an outline under work/outline.md."
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true                            // opt into the filesystem tools
  })
  .compile();
```

**What it adds:** The `fsPolicy` defines per-path rules (`glob`, `verb: read|write|gate|deny`). The agent gets eight file tools (read/write/edit/grep/ls/glob/delete/move) bounded by the policy. Writes under `work/` are ungated; reads only from `notes/`.

## Step 3 — spawn an isolated sub-agent with taskNode

Add a specialized research sub-agent that runs in isolation and returns a compressed report.

```ts
import { createGraph, DefaultLLMGateway, InMemoryToolRegistry, writeTodosTool, TODOS_CHANNEL } from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .fsPolicy([{ glob: "work/**", verb: "write" }])
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: "Plan with writeTodos, then delegate research to the research sub-agent."
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true
  })
  .taskNode("research", {
    objectiveChannel: TODOS_CHANNEL,  // sub-agent reads only the todos (the objective)
    reportChannel: "research_report", // report lands here
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: {
        system: "Research the objective from the todos. Return a tight summary of findings."
      },
      enableFs: true
    }
  })
  .edge("lead", "research")
  .compile();

const result = await app.run();
console.log("Research report:", result.channels.research_report);
```

**What it adds:** `taskNode` spawns one sub-agent in an **isolated context**. Only `objectiveChannel` (TODOS_CHANNEL) projects in; only `reportChannel` projects out. The sub-agent cannot see or pollute the parent's full state. Isolation is enforced by the Rust engine.

## Step 4 — overlay skills for playbooks

Equip the lead and its sub-agents with pinned and advisory skills.

```ts
import { createGraph, DefaultLLMGateway, InMemoryToolRegistry, writeTodosTool, TODOS_CHANNEL } from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .fsPolicy([{ glob: "work/**", verb: "write" }])
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: "Plan with writeTodos, delegate research, and follow any applicable playbook."
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true,
    skills: {
      namespace: "skill:acme:org",
      required: ["planning-guide@1.0.0"],  // always loaded (once granted)
      advisoryK: 2                          // + up to 2 vector-selected by task relevance
    }
  })
  .taskNode("research", {
    objectiveChannel: TODOS_CHANNEL,
    reportChannel: "research_report",
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: {
        system: "Research and cite sources. Follow the research playbook if available."
      },
      enableFs: true,
      skills: {
        namespace: "skill:acme:org",
        required: ["web-research@1.0.0"],   // research-specific pinned skill
        advisoryK: 3
      }
    }
  })
  .edge("lead", "research")
  .compile();
```

**What it adds:** `skills` injects `SKILL.md` playbooks (cheap frontmatter index always resident, body loaded on match). Each agent pins required skills (`required`) and pulls up to `advisoryK` advisory skills vector-selected by description. Skills granting capability (those with `requires:`) are withheld until approved.

## Step 5 — gate a sensitive tool with suspendForApproval

Add a sensitive tool (e.g., a publishing action) that requires human approval before execution.

```ts
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  writeTodosTool,
  TODOS_CHANNEL,
  type ToolId
} from "@ailu-ai/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

// Register a sensitive tool that requires approval.
tools.register(
  {
    id: "publish_article" as ToolId,
    name: "publish_article",
    description: "Publish the article to the live site. Sensitive action.",
    inputSchema: { parse: (v: unknown) => v },
    outputSchema: { parse: (v: unknown) => v },
    permissions: ["articles:publish"],
    requiresApproval: true,  // gated
    jsonSchema: { type: "object", properties: { title: { type: "string" } } }
  },
  async (input: { title?: string }) => {
    console.log("→ published article:", input.title);
    return { published: true, url: "https://example.com/article" };
  }
);

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .fsPolicy([{ glob: "work/**", verb: "write" }])
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: "Plan, research (via sub-agent), then publish the article when ready."
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true,
    suspendForApproval: true,  // suspend when a gated tool is reached (don't execute it)
    skills: {
      namespace: "skill:acme:org",
      required: ["planning-guide@1.0.0"],
      advisoryK: 2
    }
  })
  .taskNode("research", {
    objectiveChannel: TODOS_CHANNEL,
    reportChannel: "research_report",
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Research and cite sources." },
      enableFs: true,
      skills: { namespace: "skill:acme:org", required: ["web-research@1.0.0"], advisoryK: 3 }
    }
  })
  .edge("lead", "research")
  .compile();

// Run 1: the agent plans and researches, then reaches for `publish_article` → run suspends.
const suspended = await app.run();
console.log("Status:", suspended.status);              // "suspended"
console.log("Current node:", suspended.currentNodeId); // "lead"
const reqs = suspended.channels.agentResult?.approvalRequests ?? [];
console.log("Approval requests:", reqs);               // [{tool: "publish_article", ...}]

// A human reviews out of band, then approves.
const done = await app.approveAndResume(suspended.runId, { approvedTools: ["publish_article"] });
console.log("Resumed status:", done.status);           // "completed"
// → published article: … (printed only after approval)
```

**What it adds:** A tool marked `requiresApproval: true` triggers the approval gate. When `suspendForApproval: true` on the agent node, the run suspends the moment the agent tries to call it — the tool does **not** execute. A human approves by name, and `approveAndResume` re-runs the agent so the tool finally executes. An agent can never approve its own output (enforced by the engine).

## Step 6 — observe the run live with serveInspector

Watch the whole graph execute node-by-node in a live web inspector, with a governance lens showing exactly where and why it suspended.

```ts
import { createGraph, DefaultLLMGateway, serveInspector, InMemoryToolRegistry, writeTodosTool, TODOS_CHANNEL, type ToolId } from "@ailu-ai/graph-sdk";

// (same tools, graph setup as Step 5)

const app = createGraph({ name: "deep-agent" })
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .fsPolicy([{ glob: "work/**", verb: "write" }])
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: "Plan, research (via sub-agent), then publish the article when ready."
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true,
    suspendForApproval: true,
    skills: { namespace: "skill:acme:org", required: ["planning-guide@1.0.0"], advisoryK: 2 }
  })
  .taskNode("research", {
    objectiveChannel: TODOS_CHANNEL,
    reportChannel: "research_report",
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Research and cite sources." },
      enableFs: true,
      skills: { namespace: "skill:acme:org", required: ["web-research@1.0.0"], advisoryK: 3 }
    }
  })
  .edge("lead", "research")
  .compile();

// Run the graph and serve a live inspector.
const inspector = await serveInspector(app, {});
console.log(`Inspect at ${inspector.url}`);
await inspector.done;  // resolves when the run suspends or completes
```

Open the inspector URL in a browser. The left pane shows the **node timeline** (each node green on completion, suspended nodes highlighted as gates). The right pane is the **event log** — every state transition, tool call, decision. When the run suspends, an `explain()` panel shows **exactly why** (e.g., "waiting for approval of tool: publish_article") and a **one-click Resume** button to continue.

**What it adds:** Live observation via the event journal. The inspector reuses the engine's lifecycle events (`onEvent`), per-node checkpoints, and `explain(runId)` API, served over a lightweight HTTP + SSE bridge. No build step, no framework — just inline web ui.

## The full assembled graph

Here is the complete, production-ready governed deep agent, wiring all the pieces:

```ts
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  serveInspector,
  writeTodosTool,
  TODOS_CHANNEL,
  type ToolId
} from "@ailu-ai/graph-sdk";

// ============ Setup: Tools & Mocks ============

const tools = new InMemoryToolRegistry();
tools.register(writeTodosTool);

tools.register(
  {
    id: "publish_article" as ToolId,
    name: "publish_article",
    description: "Publish the article to the live site. Requires approval.",
    inputSchema: { parse: (v: unknown) => v },
    outputSchema: { parse: (v: unknown) => v },
    permissions: ["articles:publish"],
    requiresApproval: true,
    jsonSchema: { type: "object", properties: { title: { type: "string" } } }
  },
  async (input: { title?: string }) => {
    console.log("✓ Article published:", input.title);
    return { published: true, url: "https://example.com/article" };
  }
);

// ============ The Graph ============

const app = createGraph({ name: "content-publication" })
  // Durable channels
  .channel(TODOS_CHANNEL, { type: "json", default: [] })
  .channel("research_report", { type: "json" })

  // Governance: filesystem policy
  .fsPolicy([
    { glob: "work/**", verb: "write" },   // drafts & notes: full write
    { glob: "notes/**", verb: "read" }    // reference notes: read-only
  ])

  // ============ Lead Agent: Plan, Delegate, Publish ============
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: {
      system: `You are a content planner. Your job:
1. Break the task into clear todos with writeTodos.
2. Delegate research to the research sub-agent (it will return a report).
3. Once research is complete, prepare the article for publication.
4. When ready, call publish_article with the title.

Follow the planning guide. Write drafts and notes under work/.`
    },
    tools,
    todosChannel: TODOS_CHANNEL,
    enableFs: true,
    suspendForApproval: true,
    skills: {
      namespace: "skill:acme:org",
      required: ["planning-guide@1.0.0"],
      advisoryK: 2
    }
  })

  // ============ Sub-agent: Research (Isolated) ============
  .taskNode("research", {
    objectiveChannel: TODOS_CHANNEL,   // receives todos as objective
    reportChannel: "research_report",   // returns compressed report
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: {
        system: `You are a research specialist. Your objective is in the todos.
Research thoroughly and cite sources. Return a tight summary of findings.
Follow the web-research playbook if available.`
      },
      enableFs: true,
      skills: {
        namespace: "skill:acme:org",
        required: ["web-research@1.0.0"],
        advisoryK: 3
      }
    }
  })

  // ============ Wiring ============
  .edge("lead", "research")
  .compile();

// ============ Execution & Observation ============

async function main() {
  // Run with live inspection in a browser.
  const inspector = await serveInspector(app, {});
  console.log(`\nOpen inspector at: ${inspector.url}`);
  console.log("Watch the graph execute step-by-step, with governance highlighted.");
  console.log("When it suspends for approval, click 'Resume' to approve the publication.\n");

  await inspector.done;
  console.log("\nRun complete. Check the timeline and event log in the inspector.");
  console.log("The event journal is your audit trail — every decision is recorded and attributable.");
}

main().catch(console.error);
```

## Key governance guarantees (built in)

| Guarantee | How it works |
| --- | --- |
| **Planning is durable** | `writeTodos` writes to `TODOS_CHANNEL` in the same checkpoint as the result. Survives suspension. |
| **Filesystem is policy-bounded** | Every path is checked against `fsPolicy` — fail-closed by default. Write access is opt-in. |
| **Sub-agents are isolated** | A `taskNode` sub-agent sees only `objectiveChannel` in; only `reportChannel` out. The engine enforces isolation. |
| **Tools are gated** | A `requiresApproval: true` tool cannot execute without explicit human approval. Self-approval is rejected by the engine. |
| **Runs are checkpointed** | Every node, every suspension is a checkpoint. Resume replays from the latest checkpoint with the same outcome. |
| **Events are audited** | Every state transition, tool call, approval decision is an event in the journal. Attributed, attested, replayable. |
| **Skills are attributable** | Selected skills (pinned + advisory) are recorded per run. Capability-granting skills (those with `requires:`) are withheld until approved. |

## The middleware stack (invisible)

Governance is a **property of the runtime**, not application logic:

```
┌─────────────────────────────────────┐
│ Your code: plan, delegate, publish  │
├─────────────────────────────────────┤
│ User-tunable efficiency layer       │
│ (terse, compress, context-budget)   │
├─────────────────────────────────────┤
│ Sealed governed layer (engine)       │
│ · PII redaction                      │
│ · Approval gate                      │
│ · Filesystem policy                  │
│ · Isolation (taskNode)               │
│ · Attestation                        │
└─────────────────────────────────────┘
```

The governed layer is **engine-injected** — you cannot turn it off or add to it. You only tune the efficiency layer (profiles, middleware). An ungoverned agent is unrepresentable.

## Next

- **[Approval gates](/docs/governance/approval-gates)** — the suspend/resume mechanics in depth.
- **[Deep agents — todos & tasks](/docs/advanced-agents/deep-agents)** — writeTodos, taskNode, mapAgents, and when to use each.
- **[Governed virtual filesystem](/docs/advanced-agents/governed-filesystem)** — the path-policy and gated-write semantics.
- **[Skills — progressive disclosure](/docs/advanced-agents/skills)** — SKILL.md format and capability-granting.
- **[Observable runs](/docs/governance/observable-runs)** — event journal, explain(), the audit trail.
- **[Watch a run in the inspector](/docs/recipes/dev-inspector)** — the live web UI for governance inspection.
- **[Governed skills for a deep agent](/docs/recipes/governed-skills)** — a deep agent loading and gating playbooks.

## See also

- **[Tool approval and attestation](/docs/governance/tool-approval-and-attestation)** — who approved, when, what.
- **[Middleware & profiles](/docs/advanced-agents/middleware-and-profiles)** — profiles bundle tier + efficiency middleware.
- **[Resume across processes](/docs/recipes/resume-across-processes)** — checkpoint durability for long-running agents.
