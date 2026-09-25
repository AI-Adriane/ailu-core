---
title: "Parallel agents"
description: "One agent per item with mapAgents, then a council that ranks anonymized answers."
---

# Parallel agents

`mapAgents` summarizes a list of reviews in parallel, in input order. Then a council answers a question: members answer, reviewers rank the answers without knowing who wrote them, and a chair decides. Guide: [Multi-agent](../guides/multi-agent.md).

```ts file=packages/graph-sdk/examples/parallel-agents.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/parallel-agents.ts
```
