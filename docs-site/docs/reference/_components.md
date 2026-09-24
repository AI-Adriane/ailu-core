{/* Generated from componentCatalog by `pnpm --filter @ailu-ai/graph-sdk run gen:docs`. Do not edit. */}

### promptBuilder

Render every &#123;&#123;var&#125;&#125; placeholder from the channels into a target channel. Category: prompt. Use: `.component(id, components.promptBuilder({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `template` | `string` | yes | Template with &#123;&#123;var&#125;&#125; placeholders filled from the channels. |
| `into` | `string` | yes | Channel the rendered string is written into. |

### jsonValidator

Validate a channel value's type and required keys, writing an ok flag and an errors list. Category: validation. Use: `.component(id, components.jsonValidator({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel whose value is validated. |
| `requiredKeys` | `string[]` | no | Required object keys to assert present. |
| `expectType` | `"string" \| "number" \| "boolean" \| "object" \| "array" \| "null"` | no | Expected JSON type. |
| `okInto` | `string` | yes | Channel receiving the boolean validity flag. |
| `errorsInto` | `string` | yes | Channel receiving the string[] of validation errors. |

### outputParser

Extract the first balanced JSON object or array from a text channel. Category: parsing. Use: `.component(id, components.outputParser({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Text channel to extract the first JSON value from. |
| `into` | `string` | yes | Channel receiving the parsed value (or null when none is found). |

### router

Pick a route string from a channel value by ordered match rules (pairs with a conditional edge). Category: routing. Use: `.component(id, components.router({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel whose value is matched against the rules. |
| `rules` | `RouterRule[]` | yes | Ordered rules (&#123; equals?, contains?, route &#125;); the first match wins. |
| `defaultRoute` | `string` | yes | Route emitted when no rule matches. |
| `into` | `string` | yes | Channel the chosen route string is written into. |

### retriever

Score candidate documents against a query and keep the top-k by similarity. Category: retrieval. Use: `.component(id, components.retriever({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | yes | Channel holding the query text (falls back to this literal when the channel is empty). |
| `into` | `string` | yes | Channel receiving the top-k &#123; id, content, score &#125; array. |
| `k` | `number` | no | Number of results to keep (default 4). |
| `docs` | `RetrieverDoc[]` | yes | The corpus (&#123; id, content &#125;[]) to score against. |

### reranker

Reorder a retrieval-result array, optionally re-scoring against a query embedding. Category: retrieval. Use: `.component(id, components.reranker({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the retrieval-result array to reorder. |
| `into` | `string` | yes | Channel receiving the reordered array. |
| `query` | `string` | no | Optional channel holding query text for embedding-based re-scoring. |

### textCleaner

Normalise a text channel: strip HTML, lowercase, collapse whitespace, trim. Category: text. Use: `.component(id, components.textCleaner({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel whose text is normalised. |
| `into` | `string` | yes | Channel receiving the cleaned text. |
| `lowercase` | `boolean` | no | Lowercase the text. Defaults to false. |
| `stripHtml` | `boolean` | no | Strip &lt;...&gt; HTML tags. Defaults to false. |
| `collapseWhitespace` | `boolean` | no | Collapse runs of whitespace to a single space. Defaults to false. |
| `trim` | `boolean` | no | Trim leading/trailing whitespace. Defaults to false. |

### documentSplitter

Split a text channel into an array of chunk strings by chars or sentences. Category: text. Use: `.component(id, components.documentSplitter({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the text to split. |
| `into` | `string` | yes | Channel receiving the string[] of chunks. |
| `by` | `"chars" \| "sentences"` | yes | Split unit: sliding char windows or greedy sentence packing. |
| `size` | `number` | yes | Window size in chars or sentences. Must be &gt; 0. |
| `overlap` | `number` | no | Overlap repeated at the start of each next chunk. Defaults to 0. |

### htmlToText

Strip HTML tags from a text channel and decode the common named entities. Category: text. Use: `.component(id, components.htmlToText({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the HTML text. |
| `into` | `string` | yes | Channel receiving the tag-stripped, entity-decoded text. |

### csvParser

Parse a CSV text channel into an array of row objects (or arrays). Category: parsing. Use: `.component(id, components.csvParser({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the CSV text. |
| `into` | `string` | yes | Channel receiving the parsed rows array. |
| `delimiter` | `string` | no | Single-character cell delimiter. Defaults to ",". |
| `header` | `boolean` | no | When true (default) the first row supplies object keys; otherwise rows are arrays. |

### documentJoiner

Concatenate the array values across several channels into one merged array. Category: data. Use: `.component(id, components.documentJoiner({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | yes | Channels whose array values are concatenated in order. |
| `into` | `string` | yes | Channel receiving the merged array. |
| `dedupeBy` | `string` | no | Optional object field to de-duplicate the merged items by. |

### deduplicator

De-duplicate an array channel, keeping the first occurrence and preserving order. Category: data. Use: `.component(id, components.deduplicator({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the array to de-duplicate. |
| `into` | `string` | yes | Channel receiving the de-duplicated array. |
| `key` | `string` | no | Optional object field to compare items by (else whole-value compare). |

### truncator

Truncate a text channel to at most maxChars characters with an ellipsis. Category: text. Use: `.component(id, components.truncator({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the text to truncate. |
| `into` | `string` | yes | Channel receiving the (possibly truncated) text. |
| `maxChars` | `number` | yes | Maximum character length (the ellipsis counts against this budget). |
| `ellipsis` | `string` | no | Suffix appended when truncated. Defaults to "…". |

### regexExtractor

Extract literal-pattern matches (with ^/$ anchors) from a text channel. Category: parsing. Use: `.component(id, components.regexExtractor({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the text to match against. |
| `into` | `string` | yes | Channel receiving the match (or matches when all). |
| `pattern` | `string` | yes | Literal-substring pattern with optional leading ^ and trailing $ anchors. |
| `group` | `number` | no | Accepted for forward-compat; only 0 (the whole match) is supported. Defaults to 0. |
| `all` | `boolean` | no | When true, return every non-overlapping occurrence as an array. Defaults to false. |

### answerBuilder

Assemble a final answer string, optionally appending numbered citations. Category: text. Use: `.component(id, components.answerBuilder({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel supplying the core answer text. |
| `into` | `string` | yes | Channel receiving the assembled answer. |
| `contextFrom` | `string` | no | Optional channel holding a retrieval-result array rendered as numbered citations. |
| `template` | `string` | no | Optional &#123;&#123;answer&#125;&#125;/&#123;&#123;citations&#125;&#125; template controlling the layout. |

### fieldMapper

Remap an object channel's fields (by dotted path) into a new object. Category: data. Use: `.component(id, components.fieldMapper({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the source object. |
| `into` | `string` | yes | Channel receiving the remapped object. |
| `mapping` | `Record<string, string>` | yes | &#123; outKey: inKeyPath &#125; map; inKeyPath is a dotted path into the source. |

### fieldExtractor

Extract a scalar from a channel: follow an optional dotted path, and (finalOnly) reduce an agent reasoning trace to the text after the last "final:" marker. Category: data. Use: `.component(id, components.fieldExtractor({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the source value. |
| `into` | `string` | yes | Channel receiving the extracted scalar. |
| `path` | `string` | no | Optional dotted path descended into the from value (else the whole value). |
| `finalOnly` | `boolean` | no | When true, if the result is a string with a "final:" marker, keep only the text after the last marker (trimmed). Defaults to false. |

### bm25Retriever

Lexical BM25 ranking of a corpus against a query; keep the top-k by score. Category: retrieval. Use: `.component(id, components.bm25Retriever({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | yes | Channel holding the query text (falls back to this literal when the channel is empty). |
| `into` | `string` | yes | Channel receiving the top-k &#123; id, content, score &#125; array. |
| `k` | `number` | no | Number of results to keep (default 4). |
| `docs` | `LexicalDoc[]` | yes | The corpus (&#123; id, content &#125;[]) to rank. |
| `k1` | `number` | no | BM25 term-frequency saturation. Defaults to 1.2. |
| `b` | `number` | no | BM25 length-normalization. Defaults to 0.75. |

### keywordRetriever

Lexical keyword-overlap ranking: score each doc by the fraction of distinct query terms it contains. Category: retrieval. Use: `.component(id, components.keywordRetriever({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | yes | Channel holding the query text (falls back to this literal when the channel is empty). |
| `into` | `string` | yes | Channel receiving the top-k &#123; id, content, score &#125; array. |
| `k` | `number` | no | Number of results to keep (default 4). |
| `docs` | `LexicalDoc[]` | yes | The corpus (&#123; id, content &#125;[]) to rank. |

### sentenceWindowSplitter

Split text into overlapping windows of whole sentences (a sliding window with an explicit stride). Category: splitter. Use: `.component(id, components.sentenceWindowSplitter({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the text to split. |
| `into` | `string` | yes | Channel receiving the string[] of sentence windows. |
| `windowSize` | `number` | no | Sentences per window. Defaults to 3. |
| `stride` | `number` | no | Sentences advanced between windows (1 &lt;= stride &lt;= windowSize). Defaults to 1. |

### languageDetector

Heuristic language detection (en/fr/es/de/it/und) by stop-word hits, with an optional confidence score. Category: text. Use: `.component(id, components.languageDetector({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the text to classify. |
| `into` | `string` | yes | Channel receiving the detected language code (or "und"). |
| `confidenceInto` | `string` | no | Optional channel receiving the winning language's share of hits in [0, 1]. |

### metadataFilter

Keep the items of an array channel whose dotted-path field satisfies a predicate. Category: data. Use: `.component(id, components.metadataFilter({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the array to filter. |
| `into` | `string` | yes | Channel receiving the filtered array. |
| `field` | `string` | yes | Dotted path into each item compared by the predicate. |
| `op` | `"equals" \| "notEquals" \| "contains" \| "exists" \| "absent" \| "gt" \| "gte" \| "lt" \| "lte"` | yes | The predicate operator. |
| `value` | `unknown` | no | The comparison value (required except for exists/absent). |

### listJoiner

Combine several array channels into one list by concat, union (dedupe) or interleave. Category: data. Use: `.component(id, components.listJoiner({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | yes | Channels whose array values are combined. |
| `into` | `string` | yes | Channel receiving the combined array. |
| `mode` | `"concat" \| "union" \| "interleave"` | no | Combine mode. Defaults to "concat". |

### mergeRanker

Fuse several retrieval-result streams into one ranking with Reciprocal Rank Fusion (RRF). Category: retrieval. Use: `.component(id, components.mergeRanker({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `fromChannels` | `string[]` | yes | Channels each holding a retrieval-result array to fuse. |
| `into` | `string` | yes | Channel receiving the fused &#123; id, content, score &#125; array. |
| `idKey` | `string` | no | Object field identifying items across lists. Defaults to "id". |
| `k` | `number` | no | Keep only the top-k fused results (default: keep all). |
| `rrfK` | `number` | no | Reciprocal Rank Fusion constant. Defaults to 60. |

### evaluator

Score actual vs expected text (token-F1 / set overlap / exact match), with an optional pass flag. Category: evaluation. Use: `.component(id, components.evaluator({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `expectedFrom` | `string` | yes | Channel holding the expected/reference text. |
| `actualFrom` | `string` | yes | Channel holding the actual/candidate text. |
| `into` | `string` | yes | Channel receiving the numeric score in [0, 1]. |
| `metric` | `"tokenF1" \| "overlap" \| "exact"` | no | Scoring metric. Defaults to "tokenF1". |
| `passInto` | `string` | no | Optional channel receiving a boolean score &gt;= threshold. |
| `threshold` | `number` | no | Pass threshold for passInto. Defaults to 0.5. |

### chatMessageBuilder

Assemble a role-tagged chat-message array ([&#123; role, content &#125;]) an LLM generator consumes. Category: generation. Use: `.component(id, components.chatMessageBuilder({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `into` | `string` | yes | Channel receiving the [&#123; role, content &#125;] array. |
| `messages` | `ChatMessageSpec[]` | yes | Ordered specs (&#123; role, content?\|contentFrom? &#125;); content is rendered through the &#123;&#123;var&#125;&#125; template engine. |
| `systemFrom` | `string` | no | Optional channel prepended as a leading system message when non-empty. |

### conditionalRouter

Multi-branch rule routing over the channels by dotted-path predicates (pairs with a conditional edge). Category: routing. Use: `.component(id, components.conditionalRouter({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `into` | `string` | yes | Channel the chosen route string is written into. |
| `defaultRoute` | `string` | yes | Route emitted when no branch matches. |
| `branches` | `ConditionalRouterBranch[]` | yes | Ordered branches (&#123; when: &#123; field, op, value? &#125;, route &#125;); the first match wins. |

### documentWriter

Append documents into an in-state document store array (optionally de-duplicating by a field). Category: writer. Use: `.component(id, components.documentWriter({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `string` | yes | Channel holding the incoming documents array to append. |
| `into` | `string` | yes | Channel receiving the accumulated store array. |
| `store` | `string` | no | Channel holding the current store. Defaults to into. |
| `dedupeBy` | `string` | no | Optional object field to de-duplicate the merged store by. |

### httpFetch

Integration (vendor I/O): perform a real HTTP request via global fetch, writing &#123; status, ok, body, json &#125;. Never throws — non-2xx is surfaced via status/ok; an error/timeout writes &#123; ok: false, error &#125;. Category: integration. Use: `.node(id, components.httpFetch({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `url` | `string` | no | A literal URL to fetch (mutually exclusive with urlFrom). |
| `urlFrom` | `string` | no | A channel whose value supplies the URL (takes precedence when its channel is set). |
| `into` | `string` | yes | Channel receiving the &#123; status, ok, body, json &#125; result. |
| `method` | `string` | no | HTTP method. Defaults to "GET". |
| `headers` | `Record<string, string>` | no | Request headers sent with the call. |
| `body` | `string` | no | Request body (sent verbatim) for non-GET methods. |
| `timeoutMs` | `number` | no | Abort the request after this many milliseconds (drives an AbortController). |
| `fetchImpl` | `HttpFetchImpl` | no | The transport to call. Defaults to the real globalThis.fetch; inject a fake to stay offline. |

### webSearch

Integration (vendor I/O): run a real web search (default: Tavily connector behind TAVILY_API_KEY), writing &#123; results, note? &#125;. Degrades gracefully with no network call (empty results + note) when the key is absent. Category: integration. Use: `.node(id, components.webSearch({...}))`.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | no | A literal query (mutually exclusive with queryFrom). |
| `queryFrom` | `string` | no | A channel whose value supplies the query (takes precedence when its channel is set). |
| `into` | `string` | yes | Channel receiving the &#123; results, note? &#125; outcome. |
| `k` | `number` | no | Number of results to request. Defaults to 3. |
| `searchImpl` | `WebSearchImpl` | no | The search implementation to call. Defaults to a real Tavily connector behind TAVILY_API_KEY (no network when the key is absent). |
| `transport` | `WebSearchTransport` | no | HTTP transport the default Tavily connector posts through. Defaults to globalThis.fetch; inject a fake to stay offline. Ignored when searchImpl is supplied. |
