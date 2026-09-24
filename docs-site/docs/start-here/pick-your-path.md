---
sidebar_position: 6
title: Pick your path
description: Four ordered reading paths — one per audience. Evaluate in 5 minutes, build a deep agent, pass a governance review, or let an AI coding agent author graphs.
---

# Pick your path

Four ordered reading paths, one per audience. Each is a numbered sequence, not a link dump — follow
it top to bottom and you arrive with a working mental model.

## Evaluate Ailu in 5 minutes

You want to know what this is and see it work, fast.

1. [Try in 5 minutes](../getting-started/quickstart) — install, run a governed agent, watch it suspend at a gate and resume.
2. [Why Ailu / is it for me?](../introduction/why-ailu) — the thesis: governed by construction.
3. [How Ailu compares](../introduction/comparison) — honest about scope, against LangGraph and friends.
4. [Governed refund agent](../recipes/governed-refund-agent) — the flagship governance loop, end to end.

## Build a deep agent

You're shipping an agent that plans, delegates, and stays governed.

1. [Deep agents — start here](../advanced-agents/overview) — the roadmap and reading order.
2. [Agent nodes & ReAct](../building/agent-nodes-and-react) — the agent loop and tools.
3. [Middleware & profiles](../advanced-agents/middleware-and-profiles) — dial the whole posture in one word.
4. [The deep-agent loop](../advanced-agents/deep-agents) — writeTodos, taskNode, mapAgents.
5. [Skills](../advanced-agents/skills) — progressive-disclosure playbooks for the agent and its sub-agents.
6. [Build a governed deep agent](../recipes/governed-skills) — put it together.

## Pass a governance / compliance review

You need to know exactly how the moat holds.

1. [Governance model](../governance/governance-model) — what "governed by construction" means.
2. [Approval: human gate vs tool approval](../governance/approval-decision) — the two seams, and which you need.
3. [Tool approval & attestation](../governance/tool-approval-and-attestation) — the audit trail and no-self-approval.
4. [PII redaction](../governance/pii-redaction) — sealed, always-on scrubbing.
5. [Observable runs](../governance/observable-runs) — the event vocabulary you audit against.

## For an AI coding agent

You're a coding agent authoring Ailu graphs (or wiring one up).

1. [Built for AI agents](../reference/built-for-ai-agents) — the machine-legible surface: llms.txt, JSON Schema, the recovery loop.
2. [llms.txt](pathname:///llms.txt) — the live index of the SDK surface, served at the site root.
3. [Builder API](../reference/builder-api) — every method, with signatures.
4. [Errors](../reference/errors) — every typed error carries a `code`, a `hint`, and a `docUrl`.
