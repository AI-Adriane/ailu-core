---
sidebar_position: 1
title: Text splitters overview
description: Two pure, deterministic chunkers — documentSplitter (chars/sentences with overlap) and sentenceWindowSplitter (sliding sentence windows) — addressed by kind and wired into a graph with .component(...).
---

# Text splitters

Splitters turn a text channel into a `string[]` of chunks. They are **pure, deterministic Rust
components** — no LLM, no I/O, no config — added to a graph with `.component(id, descriptor)` from
a `components.*` factory, exactly like every other [component](/docs/building/components-reference).
Both read from a channel (`from`) and write the chunk array to another (`into`).

Two kinds ship today:

| Kind | What it does |
| --- | --- |
| `documentSplitter` | Chunk by **chars** (sliding windows) or **sentences** (greedy packing), with overlap. |
| `sentenceWindowSplitter` | A true **sliding window** of whole sentences with an explicit stride. |

```ts
import { createGraph, components } from "@ailu/graph-sdk";

const graph = createGraph({ name: "chunk-doc" })
  .channel("raw", { type: "string", default: "" })
  .channel("chunks", { type: "json", default: [] })
  .component(
    "split",
    components.documentSplitter({ from: "raw", into: "chunks", by: "chars", size: 800, overlap: 80 })
  )
  .compile();

const run = await graph.run({ raw: longText });
console.log(run.channels.chunks); // string[]
```

Empty input always yields an empty array.

## `documentSplitter`

Slices a text channel into chunk strings, by characters or by sentences.

- `by: "chars"` — slices the text by Unicode scalar value into windows of `size` characters,
  advancing `size - overlap` per step.
- `by: "sentences"` — segments on sentence terminators (`.` / `!` / `?`, terminator kept attached),
  then greedily packs whole sentences into chunks of at most `size` sentences, repeating the last
  `overlap` sentences at the start of the next chunk.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to split. |
| `into` | `string` | — | Channel receiving the `string[]` of chunks. |
| `by` | `"chars" \| "sentences"` | — | Split unit. |
| `size` | `number` | — | Window size (chars or sentences). Must be `> 0`. |
| `overlap` | `number` | `0` | Overlap repeated at the start of each next chunk. Must be `< size`. |

`size === 0` or `overlap >= size` is rejected at build time with an invalid-param error.

```ts
// Sentence-aware chunks: 5 sentences per chunk, 1 carried over.
.component(
  "split",
  components.documentSplitter({ from: "raw", into: "chunks", by: "sentences", size: 5, overlap: 1 })
)
```

## `sentenceWindowSplitter`

Splits text into **overlapping** windows of whole sentences and writes a `string[]`. Distinct from
`documentSplitter`'s sentence mode: this is a true sliding window with an explicit `stride`, so
consecutive windows share `windowSize - stride` sentences (the "sentence window" retrieval pattern).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to split. |
| `into` | `string` | — | Channel receiving the `string[]` of windows. |
| `windowSize` | `number` | `3` | Sentences per window. Must be `> 0`. |
| `stride` | `number` | `1` | Sentences advanced between windows. Must be `1 <= stride <= windowSize`. |

`windowSize === 0`, `stride === 0`, or `stride > windowSize` is rejected at build time.

```ts
// 3-sentence windows advancing 1 sentence at a time (max overlap).
.component(
  "window",
  components.sentenceWindowSplitter({ from: "raw", into: "windows", windowSize: 3, stride: 1 })
)
```

## Chaining

Splitters are pure, so they compose with the other text components. A common prep chain is
clean → split → embed/retrieve:

```ts
createGraph({ name: "prep" })
  .channel("html", { type: "string", default: "" })
  .channel("text", { type: "string", default: "" })
  .channel("chunks", { type: "json", default: [] })
  .component("strip", components.htmlToText({ from: "html", into: "text" }))
  .component("clean", components.textCleaner({ from: "text", into: "text" }))
  .component(
    "split",
    components.documentSplitter({ from: "text", into: "chunks", by: "sentences", size: 5, overlap: 1 })
  );
```

The output `string[]` feeds directly into the retrieval components — see
[`semanticRetriever` / `bm25Retriever`](/docs/reference/component-catalog#retrieval--ranking) and
the [RAG question answerer](/docs/recipes/rag-question-answerer) recipe.

## Notes & limits

- **No env vars, no provider keys.** Splitters are pure compute; nothing here reads the
  environment. Embeddings and model selection live downstream (see [Models](/docs/integrations/models/overview)).
- **Sentence segmentation is terminator-based** (`.` / `!` / `?`), not language-aware. The
  `by: "chars"` mode counts Unicode scalar values, not bytes or grapheme clusters.
- **Token-aware splitting is Planned.** The deprecated TypeScript `rag-pipeline` package shipped a
  whitespace-`TokenSplitter` (`{ chunkSize, chunkOverlap }`), but it is **not** exposed as a graph
  component and the package is deprecated in favour of the Rust engine (see
  ADR `0003-ts-engine-deprecated-sdk-on-rust`). A real tokenizer-backed splitter is not yet implemented.
- **Markdown / code / recursive-character splitting is Planned.** Only the two kinds above exist
  today.

## See also

- [Component catalog → Splitting](/docs/reference/component-catalog#splitting) — the authoritative param tables.
- [Components reference](/docs/building/components-reference) — how `.component(...)` and the `components.*` factories work.
- [RAG question answerer](/docs/recipes/rag-question-answerer) — splitters in a full retrieval flow.
