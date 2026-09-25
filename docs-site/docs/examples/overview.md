---
title: "Examples"
description: "Complete, runnable programs. Each one is run by the SDK's test suite, offline."
---

# Examples

Each example is one file you can run. They check their own results, and the SDK's tests run all
of them offline, so they work with the version these docs describe.

| Example | Shows |
| --- | --- |
| [Governed refund agent](./refund-agent.md) | An agent with a tool that needs a human's approval. |
| [Document Q&A](./document-qa.md) | Retrieval, reranking and a grounded answer with citations. |
| [Streaming chat](./streaming-chat.md) | Tokens and node progress as they happen. |
| [Resume across processes](./resume-across-processes.md) | A run that stops in one process and finishes in another. |
| [Parallel agents](./parallel-agents.md) | One agent per item, and a council that ranks its answers. |
| [Deep agent](./deep-agent.md) | Planning, a governed filesystem, and a sub-agent. |

## Run one

```bash
git clone https://github.com/AI-Adriane/ailu-core && cd ailu-core
pnpm install && bash scripts/build-napi.sh
AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/agent.ts
```

Drop `AILU_LLM_MOCK=1` and set a provider key (`ANTHROPIC_API_KEY`, ...) to run on a real model.

## Longer examples in the repository

- [`startup-e2e.ts`](https://github.com/AI-Adriane/ailu-core/blob/main/packages/graph-sdk/examples/startup-e2e.ts): a venture pipeline on the catalog runner, with a
  brand-review human gate and a `deploy_to_prod` tool that waits for the founder's approval.
- [`finance-sage-optimization.ts`](https://github.com/AI-Adriane/ailu-core/blob/main/packages/graph-sdk/examples/finance-sage-optimization.ts): an agent analyses an
  accounting export; posting corrections waits for the CFO's approval.
- [`product-pipeline.ts`](https://github.com/AI-Adriane/ailu-core/blob/main/packages/graph-sdk/examples/product-pipeline.ts): brief to launch, with a component, a
  semantic retriever and agents on three model tiers.
- [`qa-rag.ts`](https://github.com/AI-Adriane/ailu-core/blob/main/packages/graph-sdk/examples/qa-rag.ts): question answering where an answer no source backs goes to a
  human review gate instead of being published.
- [`token-economics.ts`](https://github.com/AI-Adriane/ailu-core/blob/main/packages/graph-sdk/examples/token-economics.ts): prompt tokens of one long shared context
  compared with governed RAG agents.
