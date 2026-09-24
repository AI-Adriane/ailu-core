---
title: "Quickstart"
description: "In one file, an agent drafts a reply, the run stops for a human, and resumes once approved."
---

# Quickstart

In five minutes you will build a graph where an agent drafts a reply to a customer, the run stops
for a human to review the draft, and then finishes.

## 1. Create the project

```bash
mkdir refund-desk && cd refund-desk
npm init -y && npm pkg set type=module
npm install @ailu-ai/graph-sdk
```

## 2. Write the graph

Save this as `app.ts`:

```ts file=packages/graph-sdk/examples/docs/quickstart.ts region=example title="app.ts"
```

What each part does:

- **`channel`** declares a piece of run state. Nodes read channels and return the ones they change.
- **`agentNode`** runs an LLM agent. It writes its result to the `agentResult` channel;
  `finalAnswer()` reads the answer out of it.
- **`humanGate`** stops the run. `run()` returns with status `"suspended"`.
- **`resume`** continues from the checkpoint saved at the gate.

## 3. Run it

With an Anthropic key:

```bash
ANTHROPIC_API_KEY=sk-ant-... npx tsx app.ts
```

Or offline, with the deterministic mock:

```bash
AILU_LLM_MOCK=1 npx tsx app.ts
```

You should see `suspended`, the draft, then `completed` and the reply.

## What you just used

In a real app, the gap between `run()` and `resume()` is where a person reviews the draft in your
UI. Your server keeps the `CompiledGraph` and calls `resume(runId)` when they approve. To resume
in a different process, after a deploy or a restart, see
[Long-running runs](./guides/long-running.md).

## Next

- Give the agent a tool that needs approval: [Tools and approval](./guides/tools.md).
- Understand nodes, channels and edges: [Graphs](./guides/graphs.md).
- See a complete app: [Governed refund agent](./examples/refund-agent.md).
