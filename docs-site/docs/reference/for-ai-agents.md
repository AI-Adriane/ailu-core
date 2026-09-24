---
title: "For AI coding agents"
description: "What an AI coding agent should read first, and the SDK features that let it check its own work."
---

# For AI coding agents

If an AI coding agent writes your Ailu code, give it these.

## Start here

- [`/llms.txt`](pathname:///llms.txt): a short index of the SDK, generated from the SDK itself:
  the builder, the models, every component and its parameters, the error codes.
- [`/llms-full.txt`](pathname:///llms-full.txt): this whole documentation as one text file.

`generateLlmsTxt()` returns the same index from the installed package, so it always matches the
version you have.

## Rules that save a round trip

- Configure agents with `model: model.<provider>("<id>")` or `model: "provider:model"`. `llm:`
  and `DefaultLLMGateway` are deprecated and ignored.
- Read an agent's answer with `finalAnswer(result)`.
- Name a tool's input in `jsonSchema`; the model reads it.
- Approve tools with `approveAndResume(runId, { approvedTools: ["name"], resolvedBy })`.
- `resume` works only on the `CompiledGraph` that started the run. Across processes, use
  `runCatalogGraph` and `resumeCatalogGraph`.
- Run tests with `AILU_LLM_MOCK=1`.

## Check your own work

| Tool | Tells you |
| --- | --- |
| `app.compile()` / `safeCompile()` | Whether the graph is valid, with a code and a fix for each problem. |
| `componentSchemas()` | A JSON Schema for the parameters of every component. |
| `app.explain(runId)`, `explainRun(state)` | Why a run is suspended or failed, and the exact call that continues it. |
| `error.code`, `error.hint`, `error.docUrl` | What went wrong, how to fix it, and where it is documented. See [Errors](./errors.md). |

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=explain
```
