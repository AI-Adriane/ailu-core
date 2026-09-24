/**
 * The static catalog metadata that backs the API's `/catalog` endpoint: one entry per
 * component, prebuilt agent and capability tier. This is the SDK's source of truth for
 * the building-block library; the API validates these arrays against the
 * `@ailu-ai/contracts` catalog DTOs and forwards them to Studio unchanged.
 *
 * The arrays mirror, one-for-one:
 *  - the 30 component factories in {@link import("./components.js").components} (28 pure
 *    Rust-backed components + 2 vendor-I/O integration components), with their real
 *    factory params;
 *  - the 16 prebuilt micro-agent definitions (the Rust `PrebuiltAgent` table);
 *  - the 4 capability tiers, each carrying the {@link DEFAULT_TIER_TABLE} per-provider
 *    recommended models.
 *
 * The shapes are deliberately plain data (no closures) so they map 1:1 onto the
 * contracts DTOs and serialize over the wire as-is.
 */

import { DEFAULT_TIER_TABLE, MODEL_TIERS, type ModelTier } from "@ailu-ai/llm-gateway";

import type { ComponentKind } from "./components.js";

/** The category buckets a component falls into in the library. */
export type ComponentCategory =
  | "prompt"
  | "validation"
  | "parsing"
  | "routing"
  | "retrieval"
  | "text"
  | "data"
  | "integration"
  // --- wave two: Haystack-gap categories ---
  | "splitter"
  | "generation"
  | "evaluation"
  | "writer";

/** A single parameter a component factory accepts. */
export type ComponentParamMeta = {
  name: string;
  type: string;
  required: boolean;
  description: string;
};

/** One entry in the component library: a `kind` plus its presentation + params. */
export type ComponentCatalogEntry = {
  /** The component kind — a {@link ComponentKind} for pure components, or an integration name. */
  kind: ComponentKind | "httpFetch" | "webSearch";
  title: string;
  category: ComponentCategory;
  description: string;
  params: ComponentParamMeta[];
  /** `true` for the vendor-I/O integration components (httpFetch / webSearch). */
  integration: boolean;
};

/** One entry in the prebuilt-agent catalog, mirroring the Rust `PrebuiltAgent` table. */
export type PrebuiltAgentCatalogEntry = {
  name: string;
  title: string;
  description: string;
  tier: ModelTier;
  tools: string[];
  suspendForApproval: boolean;
  outputChannel: string;
};

/** Describes one capability tier plus its recommended per-provider models. */
export type ModelTierInfo = {
  tier: ModelTier;
  description: string;
  /** `provider -> model` recommended defaults for this tier. */
  models: Record<string, string>;
};

/**
 * The 30 component catalog entries: 28 pure Rust-backed components (whose `kind`
 * matches {@link ComponentKind} / `ComponentRegistry::kinds()`) plus 2 vendor-I/O
 * integration components. Params mirror the real factory `*Params` types in
 * `./components.ts`.
 */
export const componentCatalog: readonly ComponentCatalogEntry[] = [
  {
    kind: "promptBuilder",
    title: "Prompt Builder",
    category: "prompt",
    description: "Render every {{var}} placeholder from the channels into a target channel.",
    params: [
      { name: "template", type: "string", required: true, description: "Template with {{var}} placeholders filled from the channels." },
      { name: "into", type: "string", required: true, description: "Channel the rendered string is written into." }
    ],
    integration: false
  },
  {
    kind: "jsonValidator",
    title: "JSON Validator",
    category: "validation",
    description: "Validate a channel value's type and required keys, writing an ok flag and an errors list.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel whose value is validated." },
      { name: "requiredKeys", type: "string[]", required: false, description: "Required object keys to assert present." },
      { name: "expectType", type: '"string" | "number" | "boolean" | "object" | "array" | "null"', required: false, description: "Expected JSON type." },
      { name: "okInto", type: "string", required: true, description: "Channel receiving the boolean validity flag." },
      { name: "errorsInto", type: "string", required: true, description: "Channel receiving the string[] of validation errors." }
    ],
    integration: false
  },
  {
    kind: "outputParser",
    title: "Output Parser",
    category: "parsing",
    description: "Extract the first balanced JSON object or array from a text channel.",
    params: [
      { name: "from", type: "string", required: true, description: "Text channel to extract the first JSON value from." },
      { name: "into", type: "string", required: true, description: "Channel receiving the parsed value (or null when none is found)." }
    ],
    integration: false
  },
  {
    kind: "router",
    title: "Router",
    category: "routing",
    description: "Pick a route string from a channel value by ordered match rules (pairs with a conditional edge).",
    params: [
      { name: "from", type: "string", required: true, description: "Channel whose value is matched against the rules." },
      { name: "rules", type: "RouterRule[]", required: true, description: "Ordered rules ({ equals?, contains?, route }); the first match wins." },
      { name: "defaultRoute", type: "string", required: true, description: "Route emitted when no rule matches." },
      { name: "into", type: "string", required: true, description: "Channel the chosen route string is written into." }
    ],
    integration: false
  },
  {
    kind: "retriever",
    title: "Retriever",
    category: "retrieval",
    description: "Score candidate documents against a query and keep the top-k by similarity.",
    params: [
      { name: "query", type: "string", required: true, description: "Channel holding the query text (falls back to this literal when the channel is empty)." },
      { name: "into", type: "string", required: true, description: "Channel receiving the top-k { id, content, score } array." },
      { name: "k", type: "number", required: false, description: "Number of results to keep (default 4)." },
      { name: "docs", type: "RetrieverDoc[]", required: true, description: "The corpus ({ id, content }[]) to score against." }
    ],
    integration: false
  },
  {
    kind: "reranker",
    title: "Reranker",
    category: "retrieval",
    description: "Reorder a retrieval-result array, optionally re-scoring against a query embedding.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the retrieval-result array to reorder." },
      { name: "into", type: "string", required: true, description: "Channel receiving the reordered array." },
      { name: "query", type: "string", required: false, description: "Optional channel holding query text for embedding-based re-scoring." }
    ],
    integration: false
  },
  {
    kind: "textCleaner",
    title: "Text Cleaner",
    category: "text",
    description: "Normalise a text channel: strip HTML, lowercase, collapse whitespace, trim.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel whose text is normalised." },
      { name: "into", type: "string", required: true, description: "Channel receiving the cleaned text." },
      { name: "lowercase", type: "boolean", required: false, description: "Lowercase the text. Defaults to false." },
      { name: "stripHtml", type: "boolean", required: false, description: "Strip <...> HTML tags. Defaults to false." },
      { name: "collapseWhitespace", type: "boolean", required: false, description: "Collapse runs of whitespace to a single space. Defaults to false." },
      { name: "trim", type: "boolean", required: false, description: "Trim leading/trailing whitespace. Defaults to false." }
    ],
    integration: false
  },
  {
    kind: "documentSplitter",
    title: "Document Splitter",
    category: "text",
    description: "Split a text channel into an array of chunk strings by chars or sentences.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the text to split." },
      { name: "into", type: "string", required: true, description: "Channel receiving the string[] of chunks." },
      { name: "by", type: '"chars" | "sentences"', required: true, description: "Split unit: sliding char windows or greedy sentence packing." },
      { name: "size", type: "number", required: true, description: "Window size in chars or sentences. Must be > 0." },
      { name: "overlap", type: "number", required: false, description: "Overlap repeated at the start of each next chunk. Defaults to 0." }
    ],
    integration: false
  },
  {
    kind: "htmlToText",
    title: "HTML to Text",
    category: "text",
    description: "Strip HTML tags from a text channel and decode the common named entities.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the HTML text." },
      { name: "into", type: "string", required: true, description: "Channel receiving the tag-stripped, entity-decoded text." }
    ],
    integration: false
  },
  {
    kind: "csvParser",
    title: "CSV Parser",
    category: "parsing",
    description: "Parse a CSV text channel into an array of row objects (or arrays).",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the CSV text." },
      { name: "into", type: "string", required: true, description: "Channel receiving the parsed rows array." },
      { name: "delimiter", type: "string", required: false, description: 'Single-character cell delimiter. Defaults to ",".' },
      { name: "header", type: "boolean", required: false, description: "When true (default) the first row supplies object keys; otherwise rows are arrays." }
    ],
    integration: false
  },
  {
    kind: "documentJoiner",
    title: "Document Joiner",
    category: "data",
    description: "Concatenate the array values across several channels into one merged array.",
    params: [
      { name: "fromChannels", type: "string[]", required: true, description: "Channels whose array values are concatenated in order." },
      { name: "into", type: "string", required: true, description: "Channel receiving the merged array." },
      { name: "dedupeBy", type: "string", required: false, description: "Optional object field to de-duplicate the merged items by." }
    ],
    integration: false
  },
  {
    kind: "deduplicator",
    title: "Deduplicator",
    category: "data",
    description: "De-duplicate an array channel, keeping the first occurrence and preserving order.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the array to de-duplicate." },
      { name: "into", type: "string", required: true, description: "Channel receiving the de-duplicated array." },
      { name: "key", type: "string", required: false, description: "Optional object field to compare items by (else whole-value compare)." }
    ],
    integration: false
  },
  {
    kind: "truncator",
    title: "Truncator",
    category: "text",
    description: "Truncate a text channel to at most maxChars characters with an ellipsis.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the text to truncate." },
      { name: "into", type: "string", required: true, description: "Channel receiving the (possibly truncated) text." },
      { name: "maxChars", type: "number", required: true, description: "Maximum character length (the ellipsis counts against this budget)." },
      { name: "ellipsis", type: "string", required: false, description: 'Suffix appended when truncated. Defaults to "…".' }
    ],
    integration: false
  },
  {
    kind: "regexExtractor",
    title: "Regex Extractor",
    category: "parsing",
    description: "Extract literal-pattern matches (with ^/$ anchors) from a text channel.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the text to match against." },
      { name: "into", type: "string", required: true, description: "Channel receiving the match (or matches when all)." },
      { name: "pattern", type: "string", required: true, description: "Literal-substring pattern with optional leading ^ and trailing $ anchors." },
      { name: "group", type: "number", required: false, description: "Accepted for forward-compat; only 0 (the whole match) is supported. Defaults to 0." },
      { name: "all", type: "boolean", required: false, description: "When true, return every non-overlapping occurrence as an array. Defaults to false." }
    ],
    integration: false
  },
  {
    kind: "answerBuilder",
    title: "Answer Builder",
    category: "text",
    description: "Assemble a final answer string, optionally appending numbered citations.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel supplying the core answer text." },
      { name: "into", type: "string", required: true, description: "Channel receiving the assembled answer." },
      { name: "contextFrom", type: "string", required: false, description: "Optional channel holding a retrieval-result array rendered as numbered citations." },
      { name: "template", type: "string", required: false, description: "Optional {{answer}}/{{citations}} template controlling the layout." }
    ],
    integration: false
  },
  {
    kind: "fieldMapper",
    title: "Field Mapper",
    category: "data",
    description: "Remap an object channel's fields (by dotted path) into a new object.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the source object." },
      { name: "into", type: "string", required: true, description: "Channel receiving the remapped object." },
      { name: "mapping", type: "Record<string, string>", required: true, description: "{ outKey: inKeyPath } map; inKeyPath is a dotted path into the source." }
    ],
    integration: false
  },
  {
    kind: "fieldExtractor",
    title: "Field Extractor",
    category: "data",
    description: "Extract a scalar from a channel: follow an optional dotted path, and (finalOnly) reduce an agent reasoning trace to the text after the last \"final:\" marker.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the source value." },
      { name: "into", type: "string", required: true, description: "Channel receiving the extracted scalar." },
      { name: "path", type: "string", required: false, description: "Optional dotted path descended into the from value (else the whole value)." },
      { name: "finalOnly", type: "boolean", required: false, description: "When true, if the result is a string with a \"final:\" marker, keep only the text after the last marker (trimmed). Defaults to false." }
    ],
    integration: false
  },
  {
    kind: "bm25Retriever",
    title: "BM25 Retriever",
    category: "retrieval",
    description: "Lexical BM25 ranking of a corpus against a query; keep the top-k by score.",
    params: [
      { name: "query", type: "string", required: true, description: "Channel holding the query text (falls back to this literal when the channel is empty)." },
      { name: "into", type: "string", required: true, description: "Channel receiving the top-k { id, content, score } array." },
      { name: "k", type: "number", required: false, description: "Number of results to keep (default 4)." },
      { name: "docs", type: "LexicalDoc[]", required: true, description: "The corpus ({ id, content }[]) to rank." },
      { name: "k1", type: "number", required: false, description: "BM25 term-frequency saturation. Defaults to 1.2." },
      { name: "b", type: "number", required: false, description: "BM25 length-normalization. Defaults to 0.75." }
    ],
    integration: false
  },
  {
    kind: "keywordRetriever",
    title: "Keyword Retriever",
    category: "retrieval",
    description: "Lexical keyword-overlap ranking: score each doc by the fraction of distinct query terms it contains.",
    params: [
      { name: "query", type: "string", required: true, description: "Channel holding the query text (falls back to this literal when the channel is empty)." },
      { name: "into", type: "string", required: true, description: "Channel receiving the top-k { id, content, score } array." },
      { name: "k", type: "number", required: false, description: "Number of results to keep (default 4)." },
      { name: "docs", type: "LexicalDoc[]", required: true, description: "The corpus ({ id, content }[]) to rank." }
    ],
    integration: false
  },
  {
    kind: "sentenceWindowSplitter",
    title: "Sentence Window Splitter",
    category: "splitter",
    description: "Split text into overlapping windows of whole sentences (a sliding window with an explicit stride).",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the text to split." },
      { name: "into", type: "string", required: true, description: "Channel receiving the string[] of sentence windows." },
      { name: "windowSize", type: "number", required: false, description: "Sentences per window. Defaults to 3." },
      { name: "stride", type: "number", required: false, description: "Sentences advanced between windows (1 <= stride <= windowSize). Defaults to 1." }
    ],
    integration: false
  },
  {
    kind: "languageDetector",
    title: "Language Detector",
    category: "text",
    description: "Heuristic language detection (en/fr/es/de/it/und) by stop-word hits, with an optional confidence score.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the text to classify." },
      { name: "into", type: "string", required: true, description: "Channel receiving the detected language code (or \"und\")." },
      { name: "confidenceInto", type: "string", required: false, description: "Optional channel receiving the winning language's share of hits in [0, 1]." }
    ],
    integration: false
  },
  {
    kind: "metadataFilter",
    title: "Metadata Filter",
    category: "data",
    description: "Keep the items of an array channel whose dotted-path field satisfies a predicate.",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the array to filter." },
      { name: "into", type: "string", required: true, description: "Channel receiving the filtered array." },
      { name: "field", type: "string", required: true, description: "Dotted path into each item compared by the predicate." },
      { name: "op", type: '"equals" | "notEquals" | "contains" | "exists" | "absent" | "gt" | "gte" | "lt" | "lte"', required: true, description: "The predicate operator." },
      { name: "value", type: "unknown", required: false, description: "The comparison value (required except for exists/absent)." }
    ],
    integration: false
  },
  {
    kind: "listJoiner",
    title: "List Joiner",
    category: "data",
    description: "Combine several array channels into one list by concat, union (dedupe) or interleave.",
    params: [
      { name: "fromChannels", type: "string[]", required: true, description: "Channels whose array values are combined." },
      { name: "into", type: "string", required: true, description: "Channel receiving the combined array." },
      { name: "mode", type: '"concat" | "union" | "interleave"', required: false, description: 'Combine mode. Defaults to "concat".' }
    ],
    integration: false
  },
  {
    kind: "mergeRanker",
    title: "Merge Ranker",
    category: "retrieval",
    description: "Fuse several retrieval-result streams into one ranking with Reciprocal Rank Fusion (RRF).",
    params: [
      { name: "fromChannels", type: "string[]", required: true, description: "Channels each holding a retrieval-result array to fuse." },
      { name: "into", type: "string", required: true, description: "Channel receiving the fused { id, content, score } array." },
      { name: "idKey", type: "string", required: false, description: 'Object field identifying items across lists. Defaults to "id".' },
      { name: "k", type: "number", required: false, description: "Keep only the top-k fused results (default: keep all)." },
      { name: "rrfK", type: "number", required: false, description: "Reciprocal Rank Fusion constant. Defaults to 60." }
    ],
    integration: false
  },
  {
    kind: "evaluator",
    title: "Evaluator",
    category: "evaluation",
    description: "Score actual vs expected text (token-F1 / set overlap / exact match), with an optional pass flag.",
    params: [
      { name: "expectedFrom", type: "string", required: true, description: "Channel holding the expected/reference text." },
      { name: "actualFrom", type: "string", required: true, description: "Channel holding the actual/candidate text." },
      { name: "into", type: "string", required: true, description: "Channel receiving the numeric score in [0, 1]." },
      { name: "metric", type: '"tokenF1" | "overlap" | "exact"', required: false, description: 'Scoring metric. Defaults to "tokenF1".' },
      { name: "passInto", type: "string", required: false, description: "Optional channel receiving a boolean score >= threshold." },
      { name: "threshold", type: "number", required: false, description: "Pass threshold for passInto. Defaults to 0.5." }
    ],
    integration: false
  },
  {
    kind: "chatMessageBuilder",
    title: "Chat Message Builder",
    category: "generation",
    description: "Assemble a role-tagged chat-message array ([{ role, content }]) an LLM generator consumes.",
    params: [
      { name: "into", type: "string", required: true, description: "Channel receiving the [{ role, content }] array." },
      { name: "messages", type: "ChatMessageSpec[]", required: true, description: "Ordered specs ({ role, content?|contentFrom? }); content is rendered through the {{var}} template engine." },
      { name: "systemFrom", type: "string", required: false, description: "Optional channel prepended as a leading system message when non-empty." }
    ],
    integration: false
  },
  {
    kind: "conditionalRouter",
    title: "Conditional Router",
    category: "routing",
    description: "Multi-branch rule routing over the channels by dotted-path predicates (pairs with a conditional edge).",
    params: [
      { name: "into", type: "string", required: true, description: "Channel the chosen route string is written into." },
      { name: "defaultRoute", type: "string", required: true, description: "Route emitted when no branch matches." },
      { name: "branches", type: "ConditionalRouterBranch[]", required: true, description: "Ordered branches ({ when: { field, op, value? }, route }); the first match wins." }
    ],
    integration: false
  },
  {
    kind: "documentWriter",
    title: "Document Writer",
    category: "writer",
    description: "Append documents into an in-state document store array (optionally de-duplicating by a field).",
    params: [
      { name: "from", type: "string", required: true, description: "Channel holding the incoming documents array to append." },
      { name: "into", type: "string", required: true, description: "Channel receiving the accumulated store array." },
      { name: "store", type: "string", required: false, description: "Channel holding the current store. Defaults to into." },
      { name: "dedupeBy", type: "string", required: false, description: "Optional object field to de-duplicate the merged store by." }
    ],
    integration: false
  },
  {
    kind: "httpFetch",
    title: "HTTP Fetch",
    category: "integration",
    description:
      "Integration (vendor I/O): perform a real HTTP request via global fetch, writing { status, ok, body, json }. Never throws — non-2xx is surfaced via status/ok; an error/timeout writes { ok: false, error }.",
    params: [
      { name: "url", type: "string", required: false, description: "A literal URL to fetch (mutually exclusive with urlFrom)." },
      { name: "urlFrom", type: "string", required: false, description: "A channel whose value supplies the URL (takes precedence when its channel is set)." },
      { name: "into", type: "string", required: true, description: "Channel receiving the { status, ok, body, json } result." },
      { name: "method", type: "string", required: false, description: 'HTTP method. Defaults to "GET".' },
      { name: "headers", type: "Record<string, string>", required: false, description: "Request headers sent with the call." },
      { name: "body", type: "string", required: false, description: "Request body (sent verbatim) for non-GET methods." },
      { name: "timeoutMs", type: "number", required: false, description: "Abort the request after this many milliseconds (drives an AbortController)." },
      { name: "fetchImpl", type: "HttpFetchImpl", required: false, description: "The transport to call. Defaults to the real globalThis.fetch; inject a fake to stay offline." }
    ],
    integration: true
  },
  {
    kind: "webSearch",
    title: "Web Search",
    category: "integration",
    description:
      "Integration (vendor I/O): run a real web search (default: Tavily connector behind TAVILY_API_KEY), writing { results, note? }. Degrades gracefully with no network call (empty results + note) when the key is absent.",
    params: [
      { name: "query", type: "string", required: false, description: "A literal query (mutually exclusive with queryFrom)." },
      { name: "queryFrom", type: "string", required: false, description: "A channel whose value supplies the query (takes precedence when its channel is set)." },
      { name: "into", type: "string", required: true, description: "Channel receiving the { results, note? } outcome." },
      { name: "k", type: "number", required: false, description: "Number of results to request. Defaults to 3." },
      { name: "searchImpl", type: "WebSearchImpl", required: false, description: "The search implementation to call. Defaults to a real Tavily connector behind TAVILY_API_KEY (no network when the key is absent)." },
      { name: "transport", type: "WebSearchTransport", required: false, description: "HTTP transport the default Tavily connector posts through. Defaults to globalThis.fetch; inject a fake to stay offline. Ignored when searchImpl is supplied." }
    ],
    integration: true
  }
] as const;

/**
 * The 16 prebuilt micro-agent catalog entries, mirroring the Rust `PrebuiltAgent`
 * table (`crates/components/src/prebuilt.rs`) and the SDK `prebuilt-agents.ts` `DEFS`:
 * name, tier, description, tools, suspend flag and output channel.
 */
export const prebuiltCatalog: readonly PrebuiltAgentCatalogEntry[] = [
  {
    name: "summarizer",
    title: "Summarizer",
    description: "Condenses input text into a short, faithful summary.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "summary"
  },
  {
    name: "classifier",
    title: "Classifier",
    description: "Assigns the input to exactly one label from a fixed set.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "label"
  },
  {
    name: "extractor",
    title: "Extractor",
    description: "Extracts structured fields from unstructured text as JSON.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "extracted"
  },
  {
    name: "sqlGenerator",
    title: "SQL Generator",
    description: "Generates a SQL query from a natural-language request and schema.",
    tier: "balanced",
    tools: [],
    suspendForApproval: false,
    outputChannel: "sql"
  },
  {
    name: "ragAnswerer",
    title: "RAG Answerer",
    description:
      "Answers a question grounded in retrieved documents. Composed as a graph: the " +
      "retriever component fetches candidate documents, the reranker component reorders " +
      "them, and this agent step writes a grounded answer citing the supplied context.",
    tier: "balanced",
    tools: [],
    suspendForApproval: false,
    outputChannel: "answer"
  },
  {
    name: "refundApprover",
    title: "Refund Approver",
    description:
      "Decides whether to issue a refund and routes the action through a human approval " +
      "gate before calling the refund tool.",
    tier: "balanced",
    tools: ["refund"],
    suspendForApproval: true,
    outputChannel: "refundDecision"
  },
  {
    name: "translator",
    title: "Translator",
    description: "Translates the input text into a target language, preserving meaning.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "translation"
  },
  {
    name: "sentimentAnalyzer",
    title: "Sentiment Analyzer",
    description: "Classifies the emotional tone of the input text.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "sentiment"
  },
  {
    name: "entityExtractor",
    title: "Entity Extractor",
    description: "Extracts named entities from text as a JSON array.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "entities"
  },
  {
    name: "piiRedactor",
    title: "PII Redactor",
    description: "Redacts personally identifiable information from the input text.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "redacted"
  },
  {
    name: "intentClassifier",
    title: "Intent Classifier",
    description: "Maps the input to a single conversational intent label.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "intent"
  },
  {
    name: "titleGenerator",
    title: "Title Generator",
    description: "Generates a short, descriptive title for the input text.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "title"
  },
  {
    name: "keywordExtractor",
    title: "Keyword Extractor",
    description: "Extracts the key terms from the input text as a JSON array.",
    tier: "fast",
    tools: [],
    suspendForApproval: false,
    outputChannel: "keywords"
  },
  {
    name: "questionAnswerer",
    title: "Question Answerer",
    description: "Answers a question directly and concisely from its own knowledge.",
    tier: "balanced",
    tools: [],
    suspendForApproval: false,
    outputChannel: "answer"
  },
  {
    name: "codeReviewer",
    title: "Code Reviewer",
    description: "Reviews a code snippet or diff for correctness, security, and quality.",
    tier: "frontier",
    tools: [],
    suspendForApproval: false,
    outputChannel: "review"
  },
  {
    name: "copyEditor",
    title: "Copy Editor",
    description: "Polishes prose for clarity, grammar, flow, and tone.",
    tier: "creative",
    tools: [],
    suspendForApproval: false,
    outputChannel: "edited"
  }
] as const;

/** A human-readable blurb for each capability tier. */
const TIER_DESCRIPTIONS: Record<ModelTier, string> = {
  frontier:
    "Highest-capability models for the hardest reasoning, code and analysis tasks where quality outweighs cost.",
  balanced:
    "A balanced default trading capability against latency and cost for everyday agentic work.",
  fast: "Lowest-latency, lowest-cost models for high-volume, well-scoped tasks (classification, extraction, summarisation).",
  creative:
    "Models tuned for fluent, stylistic prose — writing, editing and tone-sensitive rewriting."
};

/**
 * The 4 capability tiers, each carrying its description and the per-provider
 * recommended models from {@link DEFAULT_TIER_TABLE} (anthropic / mistral / ollama).
 * Derived from the gateway table so the catalog tracks the source of truth.
 */
export const tierCatalog: readonly ModelTierInfo[] = MODEL_TIERS.map((tier) => {
  const models: Record<string, string> = {};
  for (const [provider, table] of Object.entries(DEFAULT_TIER_TABLE)) {
    if (table !== undefined) {
      models[provider] = table[tier];
    }
  }
  return { tier, description: TIER_DESCRIPTIONS[tier], models };
});
