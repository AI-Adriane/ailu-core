---
sidebar_position: 2
title: Component catalog
description: Every pure compute component kind, its typed params and defaults, plus the vendor-I/O integration components.
---

# Component catalog

The `components` surface (`packages/graph-sdk/src/components.ts`) is a library of **pure,
deterministic, no-LLM** compute building blocks, each addressable by a string `kind` plus a
typed `params` object. Each factory returns a `ComponentDescriptor` — the `{ kind, params }`
carrier the Rust engine runs natively, plus a faithful TypeScript handler for the fallback path.
The TS library mirrors the Rust `ailu_components` crate one-for-one in kind, params and
behaviour, so the two agree byte-for-byte on ASCII input.

Add a component with [`builder.component(...)`](/docs/reference/builder-api#componentid-descriptor-options):

```ts
import { createGraph, components } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "clean" })
  .channel("raw", { type: "string", default: "<b>Hi</b>  there" })
  .channel("clean", { type: "string", default: "" })
  .component("c", components.textCleaner({ from: "raw", into: "clean", stripHtml: true, collapseWhitespace: true }))
  .compile();

const result = await app.run({ raw: "<b>Hi</b>  there" });
console.log(result.channels.clean);
```

Expected result: prints `Hi there`.

:::note Two components are pure routers
`router` and `conditionalRouter` only **write a route string into a channel**; they do not
branch the graph. Pair them with a [`conditionalEdge`](/docs/reference/builder-api#conditionaledgefrom-to-conditionname-predicate)
that reads that channel to actually route.
:::

Below, every `kind` with its params. Required params have no default; optional params show
their default.

## Text & template

### `promptBuilder`

Render every `{{var}}` placeholder from the channels into a target channel (whitespace inside
braces tolerated; unknown placeholders render empty).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `template` | `string` | — | Template with `{{var}}` placeholders. |
| `into` | `string` | — | Channel the rendered string is written into. |

### `textCleaner`

Normalise a text channel. Fixed order regardless of param order: stripHtml → lowercase →
collapseWhitespace → trim.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel whose text is normalised. |
| `into` | `string` | — | Channel receiving the cleaned text. |
| `lowercase` | `boolean` | `false` | Lowercase the text. |
| `stripHtml` | `boolean` | `false` | Strip `<…>` HTML tags. |
| `collapseWhitespace` | `boolean` | `false` | Collapse whitespace runs to a single space. |
| `trim` | `boolean` | `false` | Trim leading/trailing whitespace. |

### `htmlToText`

Strip HTML tags and decode the common named entities (`&lt;`, `&gt;`, `&quot;`, `&amp;` —
`&amp;` decoded last so `&amp;lt;` → `&lt;`).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the HTML text. |
| `into` | `string` | — | Channel receiving the tag-stripped, entity-decoded text. |

### `truncator`

Truncate a text channel to at most `maxChars` characters (the ellipsis counts against the
budget).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text. |
| `into` | `string` | — | Channel receiving the (possibly truncated) text. |
| `maxChars` | `number` | — | Maximum character length. |
| `ellipsis` | `string` | `"…"` | Suffix appended when truncated. |

### `regexExtractor`

Extract **literal-substring** matches (with optional leading `^` and trailing `$` anchors — no
character classes, quantifiers, or capture groups).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to match. |
| `into` | `string` | — | Channel receiving the match (or matches when `all`). |
| `pattern` | `string` | — | Literal pattern with optional `^`/`$` anchors. |
| `group` | `number` | `0` | Forward-compat; only `0` (whole match) is supported. |
| `all` | `boolean` | `false` | Return every non-overlapping occurrence as an array. |

## Parsing & validation

### `jsonValidator`

Validate a channel value's JSON type and/or required keys, writing an ok flag plus an errors
array.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel whose value is validated. |
| `okInto` | `string` | — | Channel receiving the `boolean` validity flag. |
| `errorsInto` | `string` | — | Channel receiving the `string[]` of errors. |
| `requiredKeys` | `string[]` | `[]` | Object keys asserted present. |
| `expectType` | `"string" \| "number" \| "boolean" \| "object" \| "array" \| "null"` | `undefined` | Expected JSON type. |

### `outputParser`

Find the first balanced JSON object or array in a text channel and parse it (skipping over
string literals so braces inside strings don't confuse depth). Writes `null` when nothing parses.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Text channel to extract the first JSON value from. |
| `into` | `string` | — | Channel receiving the parsed value (or `null`). |

### `csvParser`

Parse CSV text into row objects (or arrays). Simple line/char splitter — rows on `\n`, cells on
the delimiter, no quoted cells or embedded newlines.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the CSV text. |
| `into` | `string` | — | Channel receiving the parsed rows array. |
| `delimiter` | `string` | `","` | Single-character cell delimiter. |
| `header` | `boolean` | `true` | First row supplies object keys; else rows are arrays. |

### `languageDetector`

Heuristic stopword-based language detection over `en / fr / es / de / it`; ties break in that
fixed order, no hits yields `"und"`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to classify. |
| `into` | `string` | — | Channel receiving the language code (`"en" \| ... \| "und"`). |
| `confidenceInto` | `string` | `undefined` | Optional channel receiving the winning share of hits in `[0, 1]`. |

## Splitting

### `documentSplitter`

Split a text channel into chunk strings, by characters (sliding windows) or sentences (greedy
packing). `overlap` is repeated at the start of each next chunk.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to split. |
| `into` | `string` | — | Channel receiving the `string[]` of chunks. |
| `by` | `"chars" \| "sentences"` | — | Split unit. |
| `size` | `number` | — | Window size (chars or sentences). Must be > 0. |
| `overlap` | `number` | `0` | Overlap repeated per chunk. Must be smaller than `size`. |

### `sentenceWindowSplitter`

Split text into overlapping windows of whole sentences.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the text to split. |
| `into` | `string` | — | Channel receiving the `string[]` of windows. |
| `windowSize` | `number` | `3` | Sentences per window. |
| `stride` | `number` | `1` | Sentences advanced between windows (`1 <= stride <= windowSize`). |

## Retrieval & ranking

### `retriever`

Score candidate docs against a query with a deterministic mock embedder + cosine similarity, and
keep the top-`k`. Output items are `{ id, content, score }`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (falls back to this literal when the channel is empty). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to score. |
| `k` | `number` | `4` | Number of results to keep. |

### `semanticRetriever`

Genuine **semantic** retrieval: rank PRE-EMBEDDED chunks by cosine similarity to a PRE-EMBEDDED
query — both supplied on channels — and keep the top-`k`. Unlike `retriever` (a deterministic mock
embedder), this consumes real embeddings produced by the gateway (e.g. Mistral), so the host seeds
the corpus and query vector. The component owns only the cosine ranking. Output items are
`{ id, content, score }`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `queryEmbeddingFrom` | `string` | — | Channel holding the query's embedding (`number[]`). |
| `chunksFrom` | `string` | — | Channel holding the corpus: `{ id, content, embedding }[]`. |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `k` | `number` | `4` | Number of results to keep. |

### `bm25Retriever`

Lexical BM25 ranking of a corpus against a query; keep the top-`k`. Output items are
`{ id, content, score }`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (literal fallback). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to rank. |
| `k` | `number` | `4` | Number of results to keep. |
| `k1` | `number` | `1.2` | BM25 term-frequency saturation. |
| `b` | `number` | `0.75` | BM25 length-normalization. |

### `keywordRetriever`

Keyword-overlap ranking (fraction of distinct query terms present per doc); keep the top-`k`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `query` | `string` | — | Channel holding the query (literal fallback). |
| `into` | `string` | — | Channel receiving the top-`k` results. |
| `docs` | `{ id: string; content: string }[]` | — | The corpus to rank. |
| `k` | `number` | `4` | Number of results to keep. |

### `reranker`

Reorder a retrieval-result array. With a `query` channel it re-scores by embedding cosine
similarity; otherwise it sorts by each item's existing `score`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the array to reorder. |
| `into` | `string` | — | Channel receiving the reordered array. |
| `query` | `string` | `undefined` | Optional channel holding query text for embedding-based re-scoring. |

### `mergeRanker`

Fuse several retrieval streams into one ranking with Reciprocal Rank Fusion.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | — | Channels each holding a retrieval-result array to fuse. |
| `into` | `string` | — | Channel receiving the fused `{ id, content, score }` array. |
| `idKey` | `string` | `"id"` | Object field identifying items across lists. |
| `k` | `number` | keep all | Keep only the top-`k` fused results. |
| `rrfK` | `number` | `60` | Reciprocal Rank Fusion constant. |

## Array & document plumbing

### `deduplicator`

De-duplicate an array channel, keeping the first occurrence and preserving order.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the array to de-duplicate. |
| `into` | `string` | — | Channel receiving the de-duplicated array. |
| `key` | `string` | `undefined` | Object field to compare items by (else whole-value compare). |

### `documentJoiner`

Concatenate the array values across several channels into one merged array.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | — | Channels whose array values are concatenated in order. |
| `into` | `string` | — | Channel receiving the merged array. |
| `dedupeBy` | `string` | `undefined` | Object field to de-duplicate the merged items by. |

### `listJoiner`

Combine several array channels by `concat`, `union` (dedupe), or `interleave`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | — | Channels whose array values are combined. |
| `into` | `string` | — | Channel receiving the combined array. |
| `mode` | `"concat" \| "union" \| "interleave"` | `"concat"` | Combine mode. |

### `metadataFilter`

Filter an array channel by a dotted-path predicate. The operator vocabulary is
`equals \| notEquals \| contains \| exists \| absent \| gt \| gte \| lt \| lte`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the array to filter. |
| `into` | `string` | — | Channel receiving the filtered array. |
| `field` | `string` | — | Dotted path into each item. |
| `op` | `PredicateOp` | — | The predicate operator. |
| `value` | `unknown` | `undefined` | Comparison value (required except for `exists`/`absent`). |

### `documentWriter`

Append documents into an in-state document store array (optionally de-duplicating).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the incoming documents array to append. |
| `into` | `string` | — | Channel receiving the accumulated store array. |
| `store` | `string` | `into` | Channel holding the current store. |
| `dedupeBy` | `string` | `undefined` | Object field to de-duplicate the merged store by. |

## Object & field mapping

### `fieldMapper`

Remap an object channel's fields (by dotted path) into a new object. Missing paths map to
`null`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the source object. |
| `into` | `string` | — | Channel receiving the remapped object. |
| `mapping` | `Record<string, string>` | — | `{ outKey: inKeyDottedPath }` map. |

### `fieldExtractor`

Extract a scalar from a channel, optionally descending a dotted path. `finalOnly` reduces an
agent reasoning trace (a string containing `final:`) to the text after the **last** `final:`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel holding the source value. |
| `into` | `string` | — | Channel receiving the extracted scalar. |
| `path` | `string` | `undefined` | Dotted path into the value (else the whole value). |
| `finalOnly` | `boolean` | `false` | For strings, return only the text after the last `final:` marker. |

## Answer & message assembly

### `answerBuilder`

Assemble a final answer string, optionally appending numbered citations from a retrieval-result
array.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel supplying the core answer text. |
| `into` | `string` | — | Channel receiving the assembled answer. |
| `contextFrom` | `string` | `undefined` | Channel holding a retrieval-result array, rendered as numbered citations. |
| `template` | `string` | `undefined` | Optional `{{answer}}` / `{{citations}}` template controlling the layout. |

### `chatMessageBuilder`

Assemble a role-tagged `[{ role, content }]` chat-message array an LLM generator consumes. Each
spec's `content` is rendered through the `{{var}}` engine; `contentFrom` reads a channel
verbatim.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `into` | `string` | — | Channel receiving the message array. |
| `messages` | `ChatMessageSpec[]` | — | Ordered message specs (see below). |
| `systemFrom` | `string` | `undefined` | Channel prepended as a leading system message when non-empty. |

`ChatMessageSpec`: `{ role: "system" | "user" | "assistant"; content?: string; contentFrom?: string }`.

## Routing (pure predicates)

:::note These write a route, they do not branch
Both `router` and `conditionalRouter` are pure predicates that **write a route string into a
channel**. They never change the graph's control flow on their own — pair them with a
`conditionalEdge` reading the route channel.
:::

### `router`

Pick a route from a channel value against ordered rules (first match wins).

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `from` | `string` | — | Channel whose value is matched. |
| `rules` | `RouterRule[]` | — | Ordered rules; first match wins. |
| `defaultRoute` | `string` | — | Route emitted when no rule matches. |
| `into` | `string` | — | Channel the chosen route string is written into. |

`RouterRule`: `{ equals?: string; contains?: string; route: string }`. Both predicates must
hold when both are set; a rule with neither set never matches.

### `conditionalRouter`

Multi-branch rule routing over the channels (first matching branch wins). Each branch's `when`
is a `{ field, op, value? }` predicate (same `PredicateOp` vocabulary as `metadataFilter`),
`field` is a dotted path into the channels.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `into` | `string` | — | Channel the chosen route string is written into. |
| `defaultRoute` | `string` | — | Route emitted when no branch matches. |
| `branches` | `ConditionalRouterBranch[]` | — | Ordered branches; first match wins. |

`ConditionalRouterBranch`: `{ when: { field: string; op: PredicateOp; value?: unknown }; route: string }`.

## Evaluation

### `evaluator`

Score actual vs expected text, with an optional pass flag.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `expectedFrom` | `string` | — | Channel holding the expected/reference text. |
| `actualFrom` | `string` | — | Channel holding the actual/candidate text. |
| `into` | `string` | — | Channel receiving the numeric score in `[0, 1]`. |
| `metric` | `"tokenF1" \| "overlap" \| "exact"` | `"tokenF1"` | Scoring metric. |
| `passInto` | `string` | `undefined` | Optional channel receiving `score >= threshold`. |
| `threshold` | `number` | `0.5` | Pass threshold for `passInto`. |

## Integration components (vendor I/O)

These two are **not** pure Rust components: they have no `{ kind, params }` carrier and return a
plain `NodeHandler` (a closure over an injected I/O impl). Add them with
[`node(...)`](/docs/reference/builder-api#nodeid-handlerorconfig), not `component(...)`. On the
Rust engine they run over the async JS seam like any other JS handler.

```ts
createGraph({ name: "fetch" })
  .channel("body", { type: "json", default: null })
  .node("get", components.httpFetch({ url: "https://example.com", into: "body", fetchImpl: fakeFetch }));
```

:::warning Network access and graceful degradation
Both default to the real `globalThis.fetch`. They **never throw** — a failure is surfaced as
data so a graph degrades gracefully. Inject `fetchImpl` / `searchImpl` / `transport` to stay
offline in tests.
:::

### `httpFetch`

Perform an HTTP request, writing an `HttpFetchResult` to `into`. On a completed response:
`{ status, ok, body, json? }` (`json` present only when the `content-type` is JSON). On a
transport error / timeout: `{ ok: false, error }`.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `into` | `string` | — | Channel receiving the `HttpFetchResult`. |
| `url` | `string` | `undefined` | A literal URL (mutually exclusive with `urlFrom`). |
| `urlFrom` | `string` | `undefined` | Channel supplying the URL (takes precedence when its channel is set). |
| `method` | `string` | `"GET"` | HTTP method. |
| `headers` | `Record<string, string>` | `undefined` | Request headers. |
| `body` | `string` | `undefined` | Request body (verbatim) for non-GET methods. |
| `timeoutMs` | `number` | `undefined` | Abort after this many ms (drives an `AbortController`). |
| `fetchImpl` | `HttpFetchImpl` | `globalThis.fetch` | Injectable transport (`Response`-shaped). |

### `webSearch`

Run a web search, writing a `WebSearchOutcome` (`{ results, note? }`) to `into`. The default is
a real Tavily connector behind `TAVILY_API_KEY`: when the key is set it POSTs to
`https://api.tavily.com/search` and normalizes results to `[{ title, url, snippet }]`; when the
key is **absent it makes no network call** and returns empty results plus a note.

| Param | Type | Default | Meaning |
| --- | --- | --- | --- |
| `into` | `string` | — | Channel receiving the `WebSearchOutcome`. |
| `query` | `string` | `undefined` | A literal query (mutually exclusive with `queryFrom`). |
| `queryFrom` | `string` | `undefined` | Channel supplying the query (takes precedence when its channel is set). |
| `k` | `number` | `3` | Number of results to request. |
| `searchImpl` | `WebSearchImpl` | Tavily connector | Injectable search implementation. |
| `transport` | `WebSearchTransport` | `globalThis.fetch` | HTTP transport the default Tavily connector posts through (ignored when `searchImpl` is supplied). |

## Next

- [Builder API](/docs/reference/builder-api)
- [Events and streams](/docs/reference/events-and-streams)
