---
sidebar_position: 18
title: Governed council for a high-stakes decision
description: N member agents answer independently, reviewers rank the ANONYMIZED field so nobody favours their own answer, and a chair synthesizes — with an optional human gate before the verdict lands.
tags: ["agents", "governance", "orchestration"]
difficulty: advanced
---

# Governed council for a high-stakes decision

A **council** (ADR 0013) is the governed version of Karpathy's llm-council pattern: dispatch a
query to N member agents, have reviewers **rank the anonymized answers** (so a reviewer can never
favour its own), aggregate the rankings (Borda count), and have a **chair** synthesize the final
answer — optionally pausing on a human gate before the verdict ships. Reserve it for questions
where paying N× the cost of one answer is worth it: pricing decisions, policy exceptions, anything
you'd want a second (and third) opinion on before it goes out.

`council(...)` builds the whole thing as a **catalog graph** — every step (dispatch, anonymize,
reviewers, aggregate, chair) is a real node with its own checkpoint and event, so it runs on the
Rust engine exactly like any other governed graph, via `runCatalogGraph`.

## 1. Build the council

```ts
import { council, runCatalogGraph, model } from "@ailu-ai/graph-sdk";

const definition = council({
  members: [
    { model: model.openai("gpt-4o"), prompt: { system: "Answer the pricing question." } },
    { model: model.anthropic("claude-sonnet-4-5"), prompt: { system: "Answer the pricing question." } },
    { model: model.openai("gpt-4o-mini"), prompt: { system: "Answer the pricing question." } }
  ],
  // reviewers default to one per member; each ranks the ANONYMIZED answers — it can never
  // recognize (or favour) its own
  chair: {
    model: model.anthropic("claude-sonnet-4-5"),
    prompt: { system: "Synthesize the best-supported answer from the ranked field." }
  },
  humanGate: true // suspend for accept/override before the chair synthesizes — high-stakes
});
```

## 2. Run it

```ts
const outcome = await runCatalogGraph(definition, {
  initialData: { query: "Should the EU tier be priced in EUR or USD, and at what point?" }
});

// humanGate: true → the run suspends before the chair. A human reviews the ranked field, then:
// const final = await resumeCatalogGraph(definition, outcome.state, { approvedTools: [...] });
```

The graph runs `dispatch → members (fan-out) → anonymize+shuffle → reviewers (fan-out, rank) →
aggregate (Borda) → [human gate] → chair`.

## What governance adds over a hand-rolled ensemble script

- **Checkpoint after every seat** — a timed-out member resumes without re-paying the others.
- **A node event per member/reviewer/chair** — who answered, who ranked whom, what the chair
  actually used. A signable audit trail of the whole deliberation, not just the final answer.
- **No self-review, by construction** — a member is never one of its own reviewers; rankings are
  computed over anonymized, relabeled (`A`/`B`/`C`…), deterministically-shuffled content, so a
  reviewer structurally cannot recognize its own answer to favour it.
- **Optional human gate before the verdict** — `humanGate: true` suspends the run right before the
  chair synthesizes, same suspend/resume mechanics as any other gated node.
- **Deterministic, replay-faithful aggregation** — the anonymize-shuffle and Borda-count steps are
  pure functions of the seed + member answers; the same run replays to the same ranking.

N is fixed by the `members` list at build time (not a runtime-sized array — see
[Parallel fan-out](/docs/recipes/parallel-fan-out) for that shape). Cost is N× one answer: reserve
council for decisions where that's worth it.

## See also

- [Hierarchical delegation (fanOut)](/docs/recipes/hierarchical-delegation) — the primitive `council()` is built on, exposed directly for a manager/specialists shape.
- [Multi-agent orchestration](/docs/building/multi-agent-orchestration) — the full reference on council and the other multi-agent patterns.
- [Approval gates](/docs/governance/approval-gates) — the human-gate mechanics `humanGate: true` uses.
