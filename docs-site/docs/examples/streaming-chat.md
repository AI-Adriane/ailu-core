---
title: "Streaming chat"
description: "Show an agent's answer token by token, and a run's progress node by node."
---

# Streaming chat

Two ways to watch a run: the `messages` stream for text as the model writes it, and the `updates` stream for one event per finished node. Guide: [Streaming](../guides/streaming.md).

```ts file=packages/graph-sdk/examples/streaming.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/streaming.ts
```
