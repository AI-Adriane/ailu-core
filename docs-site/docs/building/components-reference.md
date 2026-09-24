---
sidebar_position: 5
title: Component catalog
description: The pure, deterministic components — addressed by kind and params.
---

# Component catalog

Components are **pure, deterministic** building blocks — no LLM — addressed by a `kind` and
`params`, run natively on the Rust engine. Add one with
`.component(id, descriptor)` from a `components.*` factory, or list every kind the engine knows
with `list_components()` (Python) / the `components` export (TypeScript).

```ts
import { createGraph, components } from "@ailu-ai/graph-sdk";

createGraph({ name: "prep" })
  .channel("name", { type: "string", default: "" })
  .channel("prompt", { type: "string", default: "" })
  .component("build", components.promptBuilder({ template: "Hi {{name}}!", into: "prompt" }));
```

## Common kinds

| Kind | Purpose |
| --- | --- |
| `promptBuilder` | Render `{{var}}` placeholders into a channel. |
| `textCleaner` | Normalise text (strip HTML, lowercase, collapse whitespace, trim). |
| `documentSplitter` | Chunk text by chars or sentences, with overlap. |
| `retriever` | Mock-embedding top-`k` over a corpus. |
| `reranker` / `mergeRanker` | Reorder / merge ranked results. |
| `bm25Retriever` / `keywordRetriever` | Lexical retrieval. |
| `router` / `conditionalRouter` | Route on a **pure predicate** (never `eval`'d). |
| `jsonValidator` / `outputParser` | Validate / parse structured output. |
| `csvParser` / `htmlToText` / `regexExtractor` | Extract from raw formats. |
| `fieldMapper` / `fieldExtractor` / `metadataFilter` | Reshape records. |
| `deduplicator` / `truncator` / `listJoiner` / `documentJoiner` | List/text utilities. |
| `languageDetector` / `evaluator` / `answerBuilder` / `chatMessageBuilder` | Misc compute. |

The authoritative list and each factory's typed params live in
`engine/packages/graph-sdk/src/components.ts`. The same kinds are reachable from Python via
`run_component(kind, params, channels)`.

:::tip Router safety
`router` and `conditionalRouter` evaluate **named pure predicates**, not arbitrary code — the
same guarantee as [conditional edges](./action-nodes-and-routing#conditional-routing).
:::

## Vendor-I/O components

`httpFetch` and `webSearch` are **integrations**, not pure Rust components. They return a node
handler (a closure over an injected I/O impl) and are added with `.node(...)`, not
`.component(...)`. Inject a fake impl to keep tests offline.

## See also

- [Tools and tool nodes](./tools-and-tool-nodes)
- [Python SDK → run_component](/docs/sdk-parity/python-sdk#run-a-pure-component)
