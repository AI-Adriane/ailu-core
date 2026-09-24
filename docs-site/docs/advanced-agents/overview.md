---
sidebar_position: 0
title: Deep agents — start here
description: Governed deep agents combine planning, isolated sub-agents, a virtual filesystem, and progressive-disclosure skills — the Ailu approach to multi-turn, multi-agent workflows.
---

# Deep agents — start here

A **governed deep agent** is one that plans its own work, spawns isolated sub-agents for sub-tasks, keeps a scratchpad, and loads playbooks progressively — all with governance built in. It is the **multi-agent pattern** Ailu exists for.

Three primitives underpin it — each inheriting the runtime's guarantees (checkpointed, audited, human-gate-preserving):

- **`writeTodos`** — a planning tool that records a durable todo list.
- **`taskNode` / `mapAgents`** — spawn isolated sub-agents that return a compressed report.
- **the governed filesystem** — a run-scoped scratchpad, gated by a per-path policy.

Add a **`skills`** overlay and each sub-agent loads playbooks progressively, withheld until granted when they grant capability.

## Recommended reading order

Start here and follow the path top to bottom. Each page stands alone; the progression stacks one concept per page.

### 1. Middleware & profiles

**What it gives you:** Understand the middleware stack — the sealed governed layer (PII redaction, approval gate, filesystem policy) plus the user-tunable efficiency layer (profiles, compression, reflection).

Start with [Middleware & profiles](./middleware-and-profiles) to learn how governance is built in. Then pick a profile (`governed-deep` is the deep-agent default).

### 2. Governed filesystem

**What it gives you:** A virtual scratchpad. Give an agent eight file tools (read/write/edit/grep) bounded by a fail-closed per-path policy. Writes can be gated so a human approves each one.

Read [Governed virtual filesystem](./governed-filesystem) to see `enableFs` and `fsPolicy`, and how gated writes work. The `governed-deep` profile enables it for you.

### 3. Deep agents: todos & tasks

**What it gives you:** The core — `writeTodos` for planning, `taskNode` for one isolated sub-agent, `mapAgents` for N sub-agents over an array. Each inherits the run's checkpoints and approval gates.

Read [Deep agents — todos & tasks](./deep-agents) to learn how to spawn sub-agents and keep them isolated. This is where you wire the multi-agent loop.

### 4. Skills — progressive disclosure

**What it gives you:** Procedural playbooks — `SKILL.md` files with a cheap index plus a body loaded only when the task matches. Pins are always applied; advisory skills are vector-selected by relevance. A skill that grants capability is withheld until granted.

Read [Skills — progressive disclosure](./skills) to understand how sub-agents pull in playbooks, deterministically, with full provenance.

## End-to-end example

See the [Governed refund agent](../recipes/governed-refund-agent) recipe for the approval-gate flow in isolation. For a full deep agent with skills:

- [Governed skills for a deep agent](../recipes/governed-skills) — a deep agent that loads and gates playbooks.

## See also

- [Approval gates](/docs/governance/approval-gates) — the two governance seams (structural human gates and agent-native tool approval).
- [Multi-agent orchestration](/docs/building/multi-agent-orchestration) — wiring multiple agents into a flow.
- [Subgraphs](/docs/building/subgraphs) — isolating agent runs from each other.
