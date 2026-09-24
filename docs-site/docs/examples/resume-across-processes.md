---
title: "Resume across processes"
description: "A run stops at a human gate in one process, and finishes in another after approval."
---

# Resume across processes

Process 1 runs the graph with the catalog runner and saves the suspended state as JSON. A reviewer approves. Process 2 loads the state and resumes; given the approval engine, the resume refuses to continue until the reviewer has approved. Guide: [Long-running runs](../guides/long-running.md).

```ts file=packages/graph-sdk/examples/resume-across-processes.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/resume-across-processes.ts
```
