---
title: "Governed refund agent"
description: "A support agent whose refund tool waits for a named human's approval."
---

# Governed refund agent

A support agent can refund orders, but the refund tool is registered with `requiresApproval: true`. The run stops before the refund, a named person approves, and the refund runs exactly once. Guide: [Tools and approval](../guides/tools.md).

```ts file=packages/graph-sdk/examples/agent.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/agent.ts
```
