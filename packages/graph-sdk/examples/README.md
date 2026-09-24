# Ailu examples

Runnable, **self-checking** examples for `@ailu-ai/graph-sdk`. Each one checks its own
behaviour and throws on the first failed check, so they double as end-to-end tests:
`examples.test.ts` runs every file here on each `pnpm --filter @ailu-ai/graph-sdk test`.

They run **offline** with `AILU_LLM_MOCK=1` (no API key, no network): agents then answer from
the engine's deterministic mock, which calls each tool the agent declares once (with an empty
input) and answers `FINAL: done`. With a provider key set (e.g. `ANTHROPIC_API_KEY`), the same
code runs on the real model.

| Level | Example | What you'll learn | Run |
| --- | --- | --- | --- |
| Beginner | [Quickstart](./quickstart.ts) | Channels, a human gate, suspend and resume | `pnpm --filter @ailu-ai/graph-sdk example` |
| Beginner | [Agent + approval](./agent.ts) | An agent node, an approval-gated tool, `approveAndResume` | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:agent` |
| Beginner | [Streaming](./streaming.ts) | `app.stream(input, "messages")` and `"updates"` | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/streaming.ts` |
| Intermediate | [Rust validation](./rust-validation.ts) | `safeCompile()` and the Rust validator | `pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/rust-validation.ts` |
| Intermediate | [QA over your documents](./qa-rag.ts) | Retrieval QA with citations and a low-confidence human gate | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:qa` |
| Intermediate | [Doc-QA reference pipeline](./doc-qa-reference.ts) | A full catalog RAG pipeline, builder path and catalog path | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:docqa` |
| Intermediate | [Parallel agents](./parallel-agents.ts) | `mapAgents` fan-out and an LLM `council` | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/parallel-agents.ts` |
| Intermediate | [Resume across processes](./resume-across-processes.ts) | Save a suspended run as JSON, check the approval, resume it elsewhere | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/resume-across-processes.ts` |
| Advanced | [Deep agent](./deep-agent.ts) | `writeTodos` planning, the governed filesystem, `taskNode` delegation | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/deep-agent.ts` |
| Advanced | [Product pipeline](./product-pipeline.ts) | Capability tiers, a real retriever, a ship gate | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:product` |
| Advanced | [From idea to shipping](./startup-e2e.ts) | A governed pipeline on the catalog path, with an ApprovalEngine | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:startup` |
| Advanced | [Finance-ops optimization (Sage)](./finance-sage-optimization.ts) | Tools over a real-looking dataset, a CFO-approved posting | `AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:finance` |
| Reference | [Token economics](./token-economics.ts) | Why retrieval + caching + compression cut input tokens (no LLM call) | `pnpm --filter @ailu-ai/graph-sdk example:tokens` |

## The flagship examples in one paragraph each

### QA over your documents (`qa-rag.ts`)

Retrieve → answer → cite, with the governance twist plain RAG stacks lack: a conditional edge
routes any answer that carries **no** citation into a `humanGate` instead of publishing it. The
routing depends on which documents matched the question, not on the model's wording, so the
example behaves the same offline and live: a question the corpus covers publishes straight
through; one it does not cover suspends until a human resumes it.

### From idea to shipping (`startup-e2e.ts`)

A venture pipeline end to end: positioning → branding → **human brand review** → design (a
catalog component) → MVP build (agent + tool) → security audit with an **approval-gated
production deploy** → launch. Built only from agents, components and gates, the graph runs on
the catalog path (`runCatalogGraph` / `resumeCatalogGraph`); the deploy request is filed in an
`ApprovalEngine`, which records who approved it and refuses a self-approval.

### Finance-ops optimization (`finance-sage-optimization.ts`)

An analyst agent digests a mock Sage journal export (25 entries with planted issues: a
duplicated supplier invoice, a VAT inconsistency, payments 60+ days late, mostly manual
entries), computes KPIs, detects the anomalies and proposes optimizations. Posting the
correcting entries is gated: the run suspends until the CFO approves in the `ApprovalEngine`,
then resumes, posts the corrections exactly once, and a second agent writes the summary.

## Conventions these examples follow

- **Offline by default** — set `AILU_LLM_MOCK=1` and no provider key. The examples never pass a
  scripted LLM (`agentNode({ llm })` is deprecated and ignored by the engine); agents declare a
  `model` (`model.anthropic("claude-sonnet-4-6")`, `model.balanced`, …).
- **Structural checks** — the checks look at run status, where a run paused, which channels
  were written and which tools ran, never at the model's wording, so they hold offline and live.
  Read an agent's answer with `finalAnswer(result)`.
- **Governance first** — sensitive tools are `requiresApproval: true`; agents never approve
  their own requests; approvals go through `approveAndResume` (builder path) or an
  `ApprovalEngine` (catalog path).
- One mock behaviour worth knowing: a resumed agent re-runs from the start of its node, so on
  resume the mock calls the agent's ungated tools again before the newly approved one runs.
