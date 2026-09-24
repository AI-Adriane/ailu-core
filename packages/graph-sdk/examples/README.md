# Ailu tutorials

Learn Ailu by running governed agent graphs — every tutorial is **offline** (a scripted
mock LLM, no API key) and **self-verifying**: each one asserts its own behaviour and exits
non-zero on the first failed assertion, so the tutorials double as end-to-end tests.

| Level | Tutorial | What you'll learn | Run |
| --- | --- | --- | --- |
| Beginner | [Quickstart](./quickstart.ts) | Channels, a human-approval gate, suspend/resume from a checkpoint | `pnpm --filter @ailu-ai/graph-sdk example` |
| Intermediate | [Agent + approval](./agent.ts) | The agent node, native tool-calling, approve-and-resume | `pnpm --filter @ailu-ai/graph-sdk example:agent` |
| Intermediate | [Rust validation](./rust-validation.ts) | The Rust core behind `safeCompile` (with a TS fallback) | `pnpm --filter @ailu-ai/graph-sdk exec node --import tsx examples/rust-validation.ts` |
| Intermediate | [QA over your documents](./qa-rag.ts) | Retrieval QA with citations + a low-confidence human gate | `pnpm --filter @ailu-ai/graph-sdk example:qa` |
| Advanced | [From idea to shipping](./startup-e2e.ts) | A full governed venture pipeline: agents, gates, an ApprovalEngine | `pnpm --filter @ailu-ai/graph-sdk example:startup` |
| Advanced | [Optimisation des flux finance (Sage)](./finance-sage-optimization.ts) | Finance-ops optimization with gated corrective actions | `pnpm --filter @ailu-ai/graph-sdk example:finance` |

## The flagship tutorials in one paragraph each

### QA over your documents (`qa-rag.ts`)

The classic retrieval-QA flow — search a corpus, fetch the top document, answer with a
citation — with the governance twist plain RAG stacks lack: a conditional edge inspects the
answer and routes anything **without** a citation marker into a `humanGate` instead of
publishing it. The tutorial runs the graph twice (a grounded answer publishes straight
through; an ungrounded one suspends until a human resumes it) and asserts both paths.

### From idea to shipping (`startup-e2e.ts`)

A venture pipeline end to end: ideation → product spec → branding → **human brand review**
→ design → MVP build (agent + tool) → **security audit with an approval-gated production
deploy** → ship. You'll see the two governance seams Ailu offers — a structural
`humanGate` and a native agent suspension through a real `ApprovalEngine` — plus the
run-event journal that records every lifecycle transition.

### Optimisation des flux finance (`finance-sage-optimization.ts`)

A French finance-ops scenario: an analyst agent digests a mock Sage journal export (25
entries with planted issues — a duplicated supplier invoice, a VAT inconsistency, 60+ day
payment delays, mostly manual entries), computes KPIs, detects the anomalies, proposes
optimizations, and then tries to post corrective entries — a gated action that suspends the
run until the DAF approves. The run resumes, posts the corrections exactly once, and prints
a structured «Rapport d'optimisation».

## Conventions these tutorials follow

- **Offline by construction** — each agent node gets its own `DefaultLLMGateway` with a
  `MockLLMProviderAdapter` and a scripted `responses` array (tool-use turns, then a
  `FINAL:` turn). No network, no secrets.
- **Self-verifying** — explicit assertions with `console.error` + `process.exit(1)` on
  failure. CI can run them as smoke tests.
- **Governance-first** — sensitive tools are `requiresApproval: true`; agents never
  self-approve; approvals flow through a human gate or an `ApprovalEngine`.
- One mock-sequencing rule worth knowing: the scripted gateway is **stateful across
  suspend/resume**. A resumed agent re-runs and consumes the *next* scripted response, so
  a gated tool call that should execute after approval is scripted twice in a row, then
  the `FINAL:` turn.
