---
title: "Migrating"
description: "Old APIs and names, and what replaces each of them."
---

# Migrating

Older code, tutorials and AI-generated snippets may use the APIs on the left. The rest of these
docs only use the right-hand column.

## The project was renamed Ailu

| Before | Now |
| --- | --- |
| `@adriane-ai/*` npm packages | `@ailu-ai/*` |
| `npm create adriane` | `npm create @ailu-ai@latest` |
| Python `adriane` | `pip install ailu`, `import ailu` |
| `ADRIANE_*` environment variables | `AILU_*` (the old names are no longer read) |
| `ADR_*` error codes | `AILU_*` |
| `adriane` CLI | `ailu` |

## Models

| Before | Now |
| --- | --- |
| `agentNode({ llm: new DefaultLLMGateway() })` | `agentNode({ model: model.anthropic("claude-sonnet-4-6") })`. `llm` is ignored. |
| `MockLLMProviderAdapter`, scripted gateways in tests | `AILU_LLM_MOCK=1` |
| `agentNode({ provider: "openai", model: "gpt-4o" })` | `agentNode({ model: model.openai("gpt-4o") })` or `model: "openai:gpt-4o"` |
| `agentNode({ tier: "fast" })` | `agentNode({ model: model.fast })` |
| `import { openai } from "@ailu-ai/model-openai"` | `import { model } from "@ailu-ai/graph-sdk"`, then `model.openai(...)` |
| A run without an API key silently used a mock | It fails and names the variable to set. Use `AILU_LLM_MOCK=1` to run offline on purpose. |
| `streamAgentTokens(...)` | `app.stream(input, "messages")` |
| Reading the answer from the `messages` channel | `finalAnswer(out.channels.agentResult)` |

## Approvals and resume

| Before | Now |
| --- | --- |
| `approveAndResume(runId, ["refund"])` | `approveAndResume(runId, { approvedTools: ["refund"], resolvedBy: "alice@example.com" })` |
| `agentNode({ approvalEngine })` | `suspendForApproval: true` with `approveAndResume`, or the catalog runner: `runCatalogGraph(app.definition, { approvalEngine, tools })` |
| `toolNode` with a gated tool | Give the tool to an `agentNode` with `suspendForApproval: true` |
| `.checkpointer(...)`, a custom `Checkpointer` | `runCatalogGraph` / `resumeCatalogGraph` and your own storage. See [Long-running runs](../guides/long-running.md). |
| `resume` in a new process | `resumeCatalogGraph(definition, savedState)` |

## Engine and routing

| Before | Now |
| --- | --- |
| `AILU_SDK_ENGINE=ts`, the TypeScript engine | Removed. The Rust engine is the only engine. |
| A handler returning `Command { goto }` | `conditionalEdge` to route, `fanOut` or `mapAgents` to run in parallel |
| `ailu run` to execute a graph | `ailu run` is a dry run. Run your TypeScript code. |
| `@ailu-ai/knowledge`, `@ailu-ai/okf` | Not part of the public SDK. For retrieval, see [RAG](../guides/rag.md). |
