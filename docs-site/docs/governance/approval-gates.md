---
sidebar_position: 2
title: Approval gates
description: The two governance seams — structural human gates and agent-native tool approval.
---

# Approval gates

Ailu offers two seams for putting a human in the loop. Both **suspend the run cleanly** and
**resume from the latest checkpoint** — and in both, an agent **never approves its own output**.

## Seam 1 — a structural human gate

`.humanGate(id)` adds a node that suspends the run when execution reaches it. You resume after a
human approves, out of band.

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "publish-flow" })
  .channel("draft", { type: "string", default: "" })
  .channel("approved", { type: "boolean", default: false })
  .node("write", async () => ({ draft: "Hello from Ailu." }))
  .humanGate("review")                            // suspends here
  .node("publish", async () => ({ approved: true }))
  .edge("write", "review")
  .edge("review", "publish")
  .compile();

const suspended = await app.run();
console.log(suspended.status);        // "suspended"
console.log(suspended.currentNodeId); // "review"

// A human approves out of band, then you resume from the latest checkpoint.
const done = await app.resume(suspended.runId);
console.log(done.status);             // "completed"
console.log(done.channels.approved);  // true
```

Use a `humanGate` for a **structural** checkpoint: "a person must look at this step before we
continue" — publishing, deploying, routing a low-confidence answer.

:::note Engine note
`resume()` / `approveAndResume()` must follow a suspended run on the **same `CompiledGraph`
instance**, which holds the suspended state to feed back to the Rust engine. The engine ships
the [`Checkpointer` interface plus an `InMemoryCheckpointer`](/docs/core-concepts/resumability-and-approvals#durable-checkpoints);
implement that interface against a durable store (Postgres, Redis, …) — or use **Ailu
Studio**, the managed control plane — so the state is persisted and a different process can
resume.
:::

## Seam 2 — agent-native tool approval

When an agent reaches for a tool marked `requiresApproval`, set `suspendForApproval: true` on
the agent node. The **whole run suspends** the moment approval is needed — the tool does **not**
run. You grant the named tools and resume with `approveAndResume`.

```ts
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  type LLMGateway,
  type ToolId
} from "@ailu/graph-sdk";

const tools = new InMemoryToolRegistry();
tools.register(
  {
    id: "refund" as ToolId,
    name: "refund",
    description: "Issues a customer refund. Sensitive.",
    inputSchema: { parse: (v: unknown) => v },
    outputSchema: { parse: (v: unknown) => v },
    permissions: ["payments:write"],
    requiresApproval: true,                 // gated
    jsonSchema: { type: "object" }
  },
  async () => ({ ok: true })
);

const app = createGraph({ name: "support-agent" })
  .agentNode("assistant", {
    llm: mockLLM("refund"),                 // a mock that asks to call `refund`
    prompt: { system: "You are a support agent. Use tools when needed." },
    tools,
    suspendForApproval: true,
    maxIterations: 2
  })
  .compile();

// 1) The agent reaches for `refund` → run suspends for approval (tool NOT executed).
const suspended = await app.run();
console.log(suspended.status);                                        // "suspended"
console.log(suspended.channels.agentResult?.approvalRequests.length); // 1

// 2) A human grants approval; the run resumes and the tool runs.
const done = await app.approveAndResume(suspended.runId, { approvedTools: ["refund"] });
console.log(done.status);                                             // "completed"
```

`approveAndResume(runId, { approvedTools })` records the approved tool **names** and resumes;
the agent re-runs and executes exactly the tools you approved — it never re-gates them, and
never approves anything itself. The wire mechanics (the reserved channels) are in
[tool approval and attestation](./tool-approval-and-attestation).

## No self-approval

This is the rule the whole model turns on: the principal that **requests** an action and the
principal that **approves** it must be different.

- The Rust engine independently guards its approve/resume entry points (`ensure_can_resolve`),
  so a direct-engine caller cannot resolve an approval *as the requesting agent*. This guard
  ships in the open engine.
- A control plane on top — **Ailu Studio** (the managed governance platform), or one you
  build on the SDK — binds the resolver to the **authenticated principal** and rejects an
  attempt to approve a request made by that same identity (e.g. with a **`409`**) before it ever
  reaches the engine.

That two-layer enforcement is the [defense in depth](./governance-model#defense-in-depth) that
keeps the rule true even if one layer is misconfigured.

## Choosing a seam

| Use… | When |
| --- | --- |
| **`humanGate`** | A structural checkpoint in the flow — a person must look before continuing. |
| **`suspendForApproval`** | An agent might reach for a sensitive tool and must not proceed without sign-off on that specific action. |

## Next

- [Tool approval and attestation](./tool-approval-and-attestation)
- [Observable runs](./observable-runs)
