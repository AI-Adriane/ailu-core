---
title: "Document Q&A"
description: "A question-answering pipeline: clean, split, retrieve, rerank, answer, cite."
---

# Document Q&A

A complete retrieval pipeline built from components, with one agent that writes a grounded answer and a final step that adds numbered citations. The same graph is exported by the SDK as `buildDocQaReference()`. Guide: [RAG and retrieval](../guides/rag.md).

```ts file=packages/graph-sdk/examples/doc-qa-reference.ts
```

Run it offline:

```bash
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/doc-qa-reference.ts
```
