---
sidebar_position: 15
title: Governed skills for a deep agent (progressive disclosure)
description: Give a deep agent and its sub-agents SKILL.md playbooks — pinned or vector-selected, withheld until granted when they grant capability.
tags: ["agents", "governance"]
difficulty: advanced
---

# Governed skills for a deep agent

Give a deep agent **playbooks it loads progressively** (ADR 0035): a `SKILL.md` is a cheap index
(`name` + `description`) plus a body that loads **only when the task matches**. Pins are always
applied; advisory skills are vector-selected by relevance. A skill that grants capability is
**withheld until granted**. This is the LangGraph-`deepagents` pattern — governed.

See the [Skills concept page](/docs/advanced-agents/skills) for the full SKILL.md format.

## 1. A SKILL.md playbook

```markdown
---
name: refund-policy
version: 1.2.0
description: How to issue a refund — eligibility window, approvals, and the customer message.
requires: [refund]   # grants capability → approval-gated on selection
---

# Refund policy

1. Verify the order is within the 30-day window.
2. Refunds over €200 require an approval gate.
3. Send the customer the standard apology + timeline.
```

The control plane registers this version (approver+, provenance-stamped) into the tenant's skill
namespace. Versions are immutable: to change it, register `refund-policy@1.3.0`.

## 2. A lead that pins its policy + finds the rest

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

const graph = createGraph({ name: "support" })
  .agentNode("lead", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Help the customer. Follow any applicable playbook." },
    skills: {
      namespace: "skill:acme:org",
      required: ["refund-policy@1.2.0"], // always loaded (once granted)
      advisoryK: 3                        // + up to 3 vector-selected by relevance
    }
  })
  .compile();
```

Before the run, the engine embeds the seed, selects `refund-policy@1.2.0` (pinned) plus the top-3
skills whose descriptions match, and prepends their bodies to the seed — one message, no new
checkpoint path. On resume the same skills re-select (deterministic ranking + `name@version` pins).

## 3. Sub-agents carry their own playbooks (deepagents parity)

Because `taskNode`/`mapAgents` sub-agents build through the same path, each pins skills for its role:

```ts
createGraph({ name: "deep-agent" })
  .taskNode("research", {
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Research the objective; cite sources." },
      skills: { namespace: "skill:acme:org", required: ["web-research@1.0.0"] }
    }
  })
  .taskNode("write", {
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Draft the answer." },
      skills: { namespace: "skill:acme:org", required: ["house-style@2.0.0"] }
    }
  })
  .compile();
```

A planning lead plus specialized, skill-equipped sub-agents — governed by construction.

## 4. Capability-granting skills are withheld until granted

`refund-policy` declares `requires: [refund]`, so it **grants capability**. Until the run is granted
`skill:refund-policy@1.2.0` (the same approval set that gates tools), its body is **never injected** —
the agent runs without that playbook rather than receiving ungoverned capability guidance. Grant it
through the [approval gate](/docs/governance/approval-gates) and it loads on the next run/resume.

## What you get

- **Progressive disclosure** — lean index always resident, body only on match, resources lazy.
- **Deterministic + resumable** — seed-only injection, stable ranking, version + embedding-model pins.
- **Attributable** — the selected set is recorded per run for AI-Act traceability.
- **No ungoverned capability** — `requires` skills withheld until granted; redaction scrubs skill text.

## Next

- [Skills — progressive disclosure](/docs/advanced-agents/skills)
- [Deep agents — todos & tasks](/docs/advanced-agents/deep-agents)
- [Long-term agent memory](./agent-memory)
