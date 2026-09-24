# ADR 0022 — Deep-agent harness: gap analysis vs LangChain `deepagents`, and a governed positioning

- Status: Proposed (strategic direction + plan; no code commitment yet)
- Date: 2026-06-22
- Deciders: Mathieu (owner)
- Related: [ADR 0013](0013-llm-council-governed-deliberation.md) (governed deliberation primitive)

## Context

LangChain shipped **`deepagents`** (<https://docs.langchain.com/oss/python/deepagents/overview>):
an "agent harness" over LangGraph for long-horizon, multi-step tasks (the Claude-Code / Manus /
Deep-Research shape). Its primitives:

- **`write_todos`** — a planning tool; a todo list (pending/in_progress/completed) held in state.
- **`task`** — spawn an **ephemeral sub-agent** with a fresh context that runs autonomously and
  returns a single **compressed report** (isolation, no context bleed).
- **Virtual filesystem** — `ls/read_file/write_file/edit_file/glob/grep/execute`, pluggable
  backends, glob permission rules, multimodal — for **context offloading** (work that exceeds one
  context window lives on "disk", not in the prompt).
- **Context management** — Skills (`SKILL.md`, progressive), Memory (`AGENTS.md`, always loaded),
  automatic summarization, prompt caching.
- **HITL** — `interrupt_on={tool: true}` pauses before a tool call.

We assessed where Ailu stands and how to position.

## Gap analysis

| deepagents primitive | Ailu today | Gap |
| --- | --- | --- |
| Planning (`write_todos` in state) | `plan-execute` agent pattern (agents-core) | ⚠️ Have planning, but not the lightweight **model-managed todo-list-in-state tool**. Small. |
| Sub-agents (`task`: ephemeral, isolated, compressed report) | subgraphs (composition), `supervisor` / `swarm`, the **council** (ADR 0013), working-memory compression | ⚠️ Have sub-agent *patterns* (graph-authored), not a first-class **agent-callable spawn tool** returning a compressed report. Medium. |
| Virtual filesystem (ls/read/write/edit/glob/grep + permissions) | `artifact-store` (versioned artifacts), KB/RAG | ❌ **Real gap** — no agent-operated filesystem for context offloading. The biggest deepagents primitive Ailu lacks. |
| Context mgmt (summarization, caching, progressive skills) | working-memory compression, **prompt caching + LLMLingua** (ADR 0014), RAG/KB | ⚠️ Have compression + caching; lack the **SKILL.md / AGENTS.md progressive-context convention**. Small–medium. |
| HITL (`interrupt_on`) | **human gates + ApprovalEngine, no-self-approval, signed audit, AI-Act** | ✅ **Ailu wins** — governed approval, not a bare interrupt. |
| Durable execution | checkpoint after every node, resume | ✅ Parity (both use a durable runtime). |
| Governance (RBAC, audit, sovereignty) | native | ✅ Ailu unique. |

## Decision

**Build a "deep-agent harness" as a prebuilt on the governed Ailu engine** — adopt the
deepagents conveniences, but make every powerful action governed. Positioning: *deepagents gives
you long-horizon power; Ailu gives you that power **governed, auditable and sovereign**.*

Concretely, the harness = existing Ailu primitives + three new ones, all routed through the
runtime's guarantees (checkpoint after every node, events, human gates):

1. **`writeTodos` planning tool** — a todo-list channel + tool (cheap; mostly ergonomics over the
   existing plan-execute).
2. **`task` sub-agent tool** — an agent-callable spawn that runs a child graph (reuse subgraphs /
   the council fan-out) in an isolated context and returns a compressed report. Each spawn is a
   node → **checkpointed + audited** (which sub-agent ran, what it returned).
3. **Virtual filesystem primitive** — the real gap. An `fs` seam (`ls/read/write/edit/glob/grep`)
   backed by `artifact-store` (so files are **versioned + attributable** for free), with
   **per-path permission rules** that reuse the KB/RBAC model. Crucially: a `write_file` /
   `execute` to a guarded path **routes through an approval gate** — context offloading *and*
   governance, which deepagents does not offer.

Skills/Memory (`SKILL.md`/`AGENTS.md`) map onto the KB/RAG + the existing system-prompt path.

## Consequences

- A clear story: **"governed deep agents"** — the Claude-Code-shaped harness with approval gates,
  signed audit, RBAC, sovereignty, and the lightest orchestration (the benchmarks). Same bet as the
  council (ADR 0013): not a new trick, a **governed** version of a known-good pattern.
- The **virtual filesystem** is the headline new capability and the most work (an `fs` seam +
  artifact-store backend + permission/approval integration). It also unlocks more than deep agents
  (any agent gets governed, versioned scratch storage).
- `task` + `writeTodos` are largely ergonomics over subgraphs/plan-execute → cheap wins.

## Reserves / next

- Scope risk: a virtual FS + sub-agent tool + planning is a big surface. Phase it: (1) `writeTodos`
  + `task` over existing subgraph/plan-execute (cheap), (2) the governed `fs` seam (the real build),
  (3) SKILL/AGENTS progressive context over KB.
- `execute` (shell) is a security-sensitive primitive — must be gated/sandboxed or omitted in the
  OSS engine (likely an external, approval-gated seam, never in-engine).
- Decision needed before code: is the deep-agent harness an `agents-core` prebuilt (engine) or a
  Studio-level product feature? Recommendation: primitives (`fs` seam, `task`, `writeTodos`) in the
  engine; the opinionated harness assembly as a prebuilt.
