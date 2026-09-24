---
sidebar_position: 1
title: "Approval: human gate vs tool approval"
description: "Two distinct approval mechanisms—structural gates that pause the flow, and tool-level approvals that suspend when agents reach sensitive tools. Which do you need?"
---

# Approval: human gate vs tool approval

Ailu offers two **distinct seams** for requiring human approval before a run continues. Both suspend the run cleanly and resume from the latest checkpoint. The choice depends on *where* the decision point lives: in the graph structure (a gate) or in the agent's tool use (capability gating).

| If you need to… | Use… | See… |
| --- | --- | --- |
| Pause the run at a **structural checkpoint** — a person must review and approve before the flow continues. Example: publish-after-review, deploy after sign-off. | `humanGate` | [Approval gates](./approval-gates) |
| Suspend when an **agent reaches for a sensitive tool** — grant (or deny) that specific tool before the agent runs it. Example: agent asks to issue a refund; a human approves the refund tool, then the agent re-runs and executes it. | `suspendForApproval` on an agent node + `requiresApproval` on tools | [Approval gates](./approval-gates) → [Tool approval and attestation](./tool-approval-and-attestation) |

## The two mechanisms, side by side

### Structural gates (`humanGate`)

A `humanGate` node suspends the run **unconditionally** when execution reaches it — like a stop sign in the graph. It is a **structural building block**, declared in the graph definition:

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .node("write", async () => ({ draft: "…" }))
  .humanGate("review")                          // pause here, always
  .node("publish", async () => ({ published: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const suspended = await app.run();              // suspends at "review"
const done = await app.resume(suspended.runId); // human approved out of band
```

Use `humanGate` when you want a **predictable, stage-gated flow**: "this step always requires review."

### Agent-native approvals (`suspendForApproval`)

When an agent reaches for a tool marked `requiresApproval`, the run suspends **on-demand**. The suspension is **tool-driven**: a sensitive tool request triggers it, never any other reason. You mark the tool, set `suspendForApproval: true` on the agent, and the engine guards it:

```ts
import {
  createGraph,
  InMemoryToolRegistry,
  type ToolId
} from "@ailu/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Issues a customer refund.",
    requiresApproval: true,                    // mark as sensitive
    // …
    jsonSchema: { type: "object" }
  },
  async () => ({ ok: true })
);

const app = createGraph({ name: "support-agent" })
  .agentNode("assistant", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Use tools when needed." },
    tools,
    suspendForApproval: true                   // gate all sensitive tool use
  })
  .compile();

const suspended = await app.run();               // suspends when agent requests `refund`
const done = await app.approveAndResume(
  suspended.runId,
  { approvedTools: ["refund"] }                // grant the tool
);                                               // agent re-runs; `refund` executes
```

Use `suspendForApproval` when approval depends on **agent behavior** — whether the agent actually tries to use a sensitive tool — rather than a fixed flow milestone.

## No self-approval

Both mechanisms enforce the same rule: **the agent (the requester) and the human (the approver) must be different principals.**

- The engine guards this at the `approve()` / `approveAndResume()` entry points (`ensure_can_resolve` check).
- A control plane on top — **Ailu Studio** (managed governance) or one you build — binds the approver to an **authenticated principal** and rejects self-approval before it reaches the engine (defense in depth).

See [no self-approval](./approval-gates#no-self-approval) for the full story.

## Next

- [Approval gates](./approval-gates) — deep dive into both mechanisms, with builder API.
- [Tool approval and attestation](./tool-approval-and-attestation) — the wire-level mechanics and audit trail.
- [Resumability and approvals](../core-concepts/resumability-and-approvals) — how suspension and checkpoint work.
