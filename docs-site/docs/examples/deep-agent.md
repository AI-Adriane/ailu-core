---
title: "Deep agent"
description: "An agent that plans with todos, works in a governed filesystem and delegates to a sub-agent."
---

# Deep agent

A lead agent writes its plan with `writeTodos`, keeps notes in files under a policy, and hands research to an isolated sub-agent. Guide: [Deep agents](../guides/deep-agents.md).

```ts file=packages/graph-sdk/examples/deep-agent.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/deep-agent.ts
```
