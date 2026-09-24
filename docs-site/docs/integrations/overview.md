---
sidebar_position: 1
title: Integrations — the decision guide
description: Pick the right integration from "need durable state?" → Checkpointers, "secrets/virtual filesystem?" → Backends, "recall/semantic search?" → Retrievers + Vector stores, "a model provider?" → Models, and more — a routing table from need → category.
---

# Integrations — the decision guide

Ailu's runtime is pluggable at key seams: where state persists, where the virtual filesystem lives,
which LLM provider you call, how retrieval ranks documents, and where you add custom middleware. This
page is a **decision guide**: start with your need (durable resumability, secrets, embeddings) and
follow the arrow to the right category page.

The **Rust engine is the source of truth** — all integrations are typed through the `@ailu-ai/graph-sdk`
TypeScript exports or implemented via the Rust trait interfaces in `@ailu-ai/napi`. TypeScript fallbacks
exist for testing only (e.g. a mock LLM); production code targets the Rust engine exclusively.

## The decision table

Read your need in the left column, follow to the category, then visit that category's overview for
concrete examples and all available implementations.

| Need | Category | Summary |
| --- | --- | --- |
| **Durable runs** — resume across process boundaries, suspend a run and restart it days later. | [Checkpointers](/docs/integrations/checkpointers/overview) | The `Checkpointer` interface (`save` / `load` / `loadById` / `list`) binds to your store (Postgres, Redis, S3, file). Default: in-memory only. |
| **Virtual filesystem** — agent reads/writes files with governance. Where does storage live? | [Backends](/docs/integrations/backends/overview) | Artifact-store default (versioned + audited for free), HTTP external backend (cross-process durable), or Noop (disabled). Set by `AILU_FS_BACKEND_URL`. |
| **Secrets, credentials, API keys** — safe injection into runs and agent context. | [Backends](/docs/integrations/backends/overview) | Filesystem is the secret channel — use the `deny` path policy to make them invisible to the agent, readable by the runtime. |
| **Semantic search** — retrieve by embedding similarity, not just keyword match. | [Retrievers](/docs/integrations/retrievers/overview) + [Vector stores](/docs/integrations/vector-stores/overview) | Retrievers are graph components (`semanticRetriever`, `bm25Retriever`, `keywordRetriever`); vector stores persist pre-computed embeddings. Embeddings come from the LLM Gateway. |
| **Lexical retrieval** — BM25, keyword match, or mock embeddings (keyless dev/test). | [Retrievers](/docs/integrations/retrievers/overview) | `bm25Retriever`, `keywordRetriever`, and `retriever` (mock embeddings) need no external embeddings — pure, deterministic components. |
| **LLM provider** — Claude, OpenAI, Google, Mistral, local Ollama, etc. | [Models](/docs/integrations/models/overview) | One gateway per deployment; `DefaultLLMGateway` auto-selects by `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / env flags. |
| **Text chunking** — split documents before embedding or retrieval. | [Text splitters](/docs/integrations/text-splitters/overview) | Semantic and recursive splitters with configurable chunk size and overlap. Use with knowledge bases. |
| **Middleware** — compression, terse output, context budget, reflection, structured (schema-constrained) output. | [Middleware](/docs/integrations/middleware/overview) | Efficiency layer you compose (`compress`, `terse`, `contextBudget`, `reflection`, `structuredOutput`); governed layer (redaction, approval gate, fs policy) is sealed. |
| **Sandboxed code execution** — run JavaScript / Python in a sandbox. | [Sandboxes](/docs/integrations/sandboxes/overview) | External seam for code-execution providers (E2B, GVisor, etc.). The runtime routes `execute_code` tool calls to the sandbox. |
| **Tool sources** — bind an agent to tools from somewhere other than in-process code (an MCP server, an internal tool API, a plugin marketplace). | [Tool registries](/docs/integrations/tool-registries/overview) | The `ToolRegistry` interface (`register` / `resolve` / `list`) any tool source implements; `InMemoryToolRegistry` is the in-engine default, an MCP-backed registry is the reference external example. |

## Quick patterns

### Default (no-config) runtime

The **in-memory default** works for prototyping and testing:

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "demo" })
  // InMemoryCheckpointer is implicit — checkpoints die with the process
  // Artifact-store filesystem backend is implicit — files live in artifact versions
  // DefaultLLMGateway picks a provider from env (ANTHROPIC_API_KEY, etc.)
  .agentNode("worker", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Work on the task." }
  })
  .compile();

await app.run();  // suspended runs cannot be resumed (no durable store)
```

This requires no integration setup. For production, you will swap in:
- A durable `Checkpointer` (Redis, Postgres, S3) to survive process restarts
- A custom `FilesystemBackend` or HTTP backend if you need cross-worker file durability
- An explicit LLM provider if multi-provider selection is needed

### Control-plane pattern (Ailu Studio)

The control plane (Studio or self-hosted) handles integrations at the infrastructure layer, not in
your graph code:

```ts
// Your graph code stays the same — no hardcoded database URLs or provider keys
createGraph({ name: "customer-agent" })
  .agentNode("handler", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Handle the request." }
  })
  .compile();

// The control plane (Studio) wires:
// - Postgres checkpointer (durable resume)
// - Secrets backend (API keys, credentials)
// - Multi-tenant fs backend (cross-process file storage)
// - Provider routing (Claude for reasoning, GPT for code)
```

That separation keeps your graphs portable: the *same compiled graph* runs in dev (in-memory), CI
(mock provider), and production (Studio) without code changes.

## Architecture notes

### Checkpoints and resumability

Every agent node completion triggers a checkpoint. A checkpoint is plain JSON: `{ id, runId, graphState, createdAt }`. 
The default `InMemoryCheckpointer` holds them in a `Map`; a durable checkpointer hands them off to a store.

Because checkpoints are JSON-serializable and the seam is just four async methods (`save`, `load`, `loadById`, `list`),
implementing a Postgres, Redis, or S3 checkpointer is straightforward. See [Resume across processes](/docs/recipes/resume-across-processes) for a walkthrough.

### State, filesystem, secrets

The virtual filesystem (`read`, `write`, `edit`, `delete`, `ls`, `glob`, `grep`) runs through a
**backend seam**, not directly to your storage. The default backend maps files onto versioned artifacts
(audit-trail for free); you can swap in an HTTP backend for external durability or Noop to disable
entirely.

**Secrets** are handled via the backend + policy: write credentials to a `deny`-policy path, and they are
invisible to the agent (they read as "not found") but accessible to the runtime for injection.

### Middleware and governance

Middleware runs in a sealed **governed layer** (redaction, approval gate, filesystem policy) plus a
user-tunable **efficiency layer** (compression, terse, context budget, reflection, structured
output). You cannot add,
remove, or disable governed middleware — it is constructed-in. You compose efficiency middleware with
`profile` and `middleware` configs.

See [Middleware overview](/docs/integrations/middleware/overview) and
[Middleware & profiles](/docs/advanced-agents/middleware-and-profiles) for the full story.

### Embeddings and the LLM Gateway

Retrieval components never call a provider; embeddings come from the LLM Gateway's embeddings adapter,
configured by environment (e.g. `MISTRAL_API_KEY` for Mistral embeddings). The gateway is the single
seam where external LLM services are called — models, embeddings, and sandbox integration all funnel
through it.

## Integration categories

| Category | Purpose | When to visit |
| --- | --- | --- |
| [Models](/docs/integrations/models/overview) | LLM provider selection and setup | Picking Claude, OpenAI, Google, Ollama, etc. |
| [Middleware](/docs/integrations/middleware/overview) | Agent middleware catalog | Compression, terse output, context budgeting, reflection, structured output. |
| [Backends](/docs/integrations/backends/overview) | Virtual filesystem storage | File durability, cross-process fs, or disabling the fs. |
| [Checkpointers](/docs/integrations/checkpointers/overview) | Run persistence and resumability | Durable suspend/resume, cross-process checkpointing. |
| [Retrievers](/docs/integrations/retrievers/overview) | Document ranking components | Lexical (BM25, keyword) or semantic (embeddings) retrieval. |
| [Vector stores](/docs/integrations/vector-stores/overview) | Pre-computed embeddings storage | Persistent embedding vectors for semantic search. |
| [Text splitters](/docs/integrations/text-splitters/overview) | Document chunking | Split documents before embedding for RAG. |
| [Sandboxes](/docs/integrations/sandboxes/overview) | Code execution | JavaScript / Python sandbox providers (E2B, Gvisor). |
| [Tool registries](/docs/integrations/tool-registries/overview) | Tool source binding | `ToolRegistry` interface; in-engine default plus external sources (e.g. MCP). |

## Next

- [Models](/docs/integrations/models/overview) — LLM provider and gateway setup
- [Checkpointers](/docs/integrations/checkpointers/overview) — the durable store seam
- [Middleware](/docs/integrations/middleware/overview) — compression, terse output, reflection, and structured output

## See also

- [Governance model](/docs/governance/governance-model) — the sealed governed layer
- [Approval gates](/docs/governance/approval-gates) — two seams for human gates
- [Middleware & profiles](/docs/advanced-agents/middleware-and-profiles) — full middleware guide
- [Governed filesystem](/docs/advanced-agents/governed-filesystem) — agent file tools and policy