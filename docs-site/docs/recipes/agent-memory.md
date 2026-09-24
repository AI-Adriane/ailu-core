---
sidebar_position: 11
title: Long-term agent memory (recall across runs)
description: Give an agent governed long-term memory — it recalls relevant past context before a run and persists after.
tags: ["state"]
difficulty: intermediate
---

# Long-term agent memory

Give an agent **memory that capitalizes across runs** (ADR 0026): before a run it recalls relevant
past context (vector search) and injects it into the seed; after a run it persists what it learned,
**attributed** (who/what/when). Add one `memory` overlay:

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "assistant" })
  .agentNode("reply", {
    model: openai("gpt-4o"),
    prompt: { system: "Answer the user." },
    memory: { namespace: "tenant:acme:agent:assistant", topK: 5, recall: "vector" }
  })
  .compile();

// First run learns; a later run recalls it into context automatically.
await app.run({ question: "our deploy window is Tuesdays 9–11am" });
await app.run({ question: "when can I deploy?" }); // recalls the earlier fact
```

## The overlay

| field | meaning |
| --- | --- |
| `namespace` | Tenant-scoped memory partition. **Sealed** by the engine with the principal — never user-routable. |
| `topK` | How many memories to recall (default 5). |
| `recall` | `"vector"` (semantic) · `"graph"` (entity graph) · `"both"` (default). |

## What you get

- **Vector + graph** recall behind one seam; every write carries **provenance**.
- Recall is a **seed-inject** — it never changes run state, so determinism + resume are intact.
- Governed: installs sealed, can't be routed around.

The OSS engine recalls/persists **in-memory** (across runs within a process). The control plane
swaps a **Neo4j**-backed store (native vector index + entity graph) behind the same seam, with
governed LLM entity-extraction. See [ADR 0026](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0026-memory-architecture-engine-studio.md).

## Actionable memory: tools the agent calls (ADR 0045)

The overlay above is **automatic** — recall before, persist after. A memory-enabled agent **also**
gets two built-in, governed tools so it can act on its memory **mid-run**:

| tool | does |
| --- | --- |
| `recallMemory({ query, topK? })` | Vector-recall the agent's active memory on demand. |
| `rememberMemory({ text })` | Persist a durable fact then and there (not just at run end). |

The tools are **sealed** with the same namespace/principal as the overlay (never user-routable). They
are **replay-safe by construction**: a write is keyed by `sha256(text)`, so re-execution on a
deterministic replay is an idempotent no-op — it can't diverge.

## Updatable, durable, and external

The control plane keeps memory **honest and current** without breaking [replay as
evidence](/docs/governance/replay-as-evidence):

- **Update = supersede.** A correction writes a *new, immutable* version + marks the old one
  `superseded`; a replay still reads the snapshot the run saw.
- **Forget = tombstone.** Governed, audit-preserving — recall skips it, the record stays.
- **External read.** Other tools/agents query an agent's memory over the control-plane API
  (tenant-scoped), and operators browse/forget it in Studio.

For the conceptual model — the four memory planes and the unified seam — see
[Memory architecture](/docs/core-concepts/memory-architecture).
