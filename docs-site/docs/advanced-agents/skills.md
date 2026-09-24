---
sidebar_position: 4
title: Skills — progressive disclosure
description: Give a deep agent and its sub-agents governed playbooks (SKILL.md) loaded progressively — pinned or vector-selected, attributable, approval-gated when they grant capability.
---

# Skills — progressive disclosure

A **skill** is procedural know-how an agent loads **progressively** (ADR 0035): a `SKILL.md` =
YAML frontmatter (`name` + `description`, a cheap always-resident index) plus a markdown **body**
loaded **on demand** only when the task matches. Skills are the **playbooks a deep agent and its
sub-agents pull in** — the LangGraph-`deepagents` shape — but governed: every selected skill is
attributable, tenant-scoped, deterministic, and (when it grants capability) approval-gated.

A skill is **data, never code**: the body is prompt/context injected into the seed, never executed.
It is distinct from a **tool** (a callable function, gated by `before_tool`) and from **RAG**
(retrieved facts) — a skill is *guidance text*.

## The SKILL.md format

```markdown
---
name: refund-policy
version: 1.2.0
description: How to issue a refund — eligibility window, approvals, and the customer message.
requires: [refund, lookup_order]   # optional: tools/profiles the body assumes (governance trigger)
---

# Refund policy

1. Verify the order is within the 30-day window with `lookup_order`.
2. Refunds over €200 require an approval gate.
3. Always send the customer the standard apology + timeline.
```

| Frontmatter field | Meaning |
| --- | --- |
| `name` | Stable kebab id. Referenced as `name@version`. |
| `version` | Semver. Versions are **immutable** — to change a skill, register a new version. |
| `description` | The load-bearing, **embeddable** task-match paragraph (this is what advisory selection ranks). |
| `requires?` | Tools/profiles the body assumes. A `requires`-bearing skill **grants capability**, so it is approval-gated on selection. |
| `resources?` | L3 bundled references (artifact ref / KB doc id / relative path), resolved on demand. |

## Add skills to an agent

```mermaid
flowchart TD
    A["L1: Frontmatter Index<br/>(name, description)<br/>Always Resident"] 
    B["Task Arrives"]
    C{"Selection<br/>Strategy"}
    D["Required Pins<br/>(explicit name@version)"]
    E["Advisory Vector Top-K<br/>(description match)"]
    F["Selected Skills Set"]
    G["Check requires Field"]
    H["Capability-Granting<br/>(requires present)"]
    I["Withheld Until<br/>Grant in Approval Set"]
    J["L2: Body Loaded<br/>to Seed"]
    K["L3: Resources<br/>Resolved on Demand"]
    L["Prepend to Seed"]
    
    A --> B
    B --> C
    C -->|required| D
    C -->|advisoryK top-k| E
    D --> F
    E --> F
    F --> G
    G -->|no requires| J
    G -->|requires present| H
    H --> I
    I --> J
    J --> K
    K --> L
```

*Skills progressive disclosure: L1 index → task match & selection (pins + advisory) → L2 body → L3 resources → prepend; capability-granting skills withheld until approved.*

The `skills` overlay selects skills from a tenant-scoped namespace and prepends their bodies to the
seed before the run. Selection is **hybrid**:

- **`required`** — explicit `name@version` pins, always loaded (the must-apply playbooks).
- **advisory** — vector top-k over skill `description`s, capped by `advisoryK` (the "find the right
  playbook" path).

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

createGraph({ name: "support" })
  .agentNode("agent", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Help the customer. Follow any applicable playbook." },
    skills: {
      namespace: "skill:acme:org",         // tenant-scoped; sealed by the engine
      required: ["refund-policy@1.2.0"],    // always loaded (when granted)
      advisoryK: 3                          // up to 3 more, vector-selected by relevance
    }
  })
  .compile();
```

| `skills` field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `namespace` | `string` | — (required) | Tenant-scoped skill partition (`skill:{tenant}:org` shared + `skill:{tenant}:agent:{id}`). Sealed by the engine — never user-routable. |
| `required` | `string[]` | `[]` | Explicit `name@version` pins. |
| `advisoryK` | `number` | `3` | Cap on advisory (vector-selected) skills. `0` = pins only. |

## Skills for deep agents (deepagents parity)

The `skills` overlay is part of an agent node's config, so it applies to **both** the main deep
agent **and** its [`mapAgents`/`taskNode` sub-agents](./deep-agents) — each is built through the same
path, so each gets its own pins + advisory scope for its role. A "research" sub-agent pins
`web-research@1`, a "writer" sub-agent pins `house-style@2` — a planning lead plus a fan-out of
specialized, skill-equipped sub-agents, governed by construction.

```ts
createGraph({ name: "deep-agent" })
  .taskNode("research", {
    subAgent: {
      llm: new DefaultLLMGateway(),
      prompt: { system: "Research the objective; cite sources." },
      skills: { namespace: "skill:acme:org", required: ["web-research@1.0.0"] }
    }
  })
  .compile();
```

## Governed by construction

- **Ungoverned injection is unrepresentable.** The skill loader sits inside the sealed governed
  layer — redaction sees the world (and scrubs any injected skill text) before a provider does, and
  a skill can never install governance.
- **Capability is approval-gated.** A selected skill carrying `requires` is **withheld** until its
  grant (`skill:{name}@{version}`) is in the run's approval set — its body never enters the context
  ungranted. (See [approval gates](/docs/governance/approval-gates).)
- **Attributable + tenant-scoped.** Registration is approver+ and provenance-stamped; selection is
  namespace-checked at the seam (a tenant without access gets an empty result, never disclosure);
  the selected set is recorded per run for AI-Act traceability.
- **Deterministic + resumable.** Seed-only injection (no new checkpoint path); deterministic ranking
  with an insertion-order tie-break; `name@version` pins + pinned embedding model + tombstone-not-
  mutate, so a run re-selects the same skills on resume.

## Next

- [Deep agents — todos & tasks](./deep-agents)
- [Middleware & profiles](./middleware-and-profiles)
- [Long-term agent memory](/docs/recipes/agent-memory)
