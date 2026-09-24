/**
 * The component library surface: pure (no-LLM) compute building blocks addressable
 * by a string `kind` plus a `params` object. Mirrors the Rust `ailu_components`
 * library (`crates/components`) one-for-one in kind, params and behaviour.
 *
 * Each factory in {@link components} returns a {@link ComponentDescriptor}: the Phase
 * C carrier the Rust engine needs (`{ kind, params }`, surfaced as the graph's
 * `componentNodes` map) **and** a faithful TypeScript `handler` for the fallback path
 * when the native addon is absent. The components are simple and pure, so the two
 * implementations agree byte-for-byte on ASCII input.
 *
 * Use {@link GraphBuilder.component} to push a node carrying both:
 *
 * ```ts
 * import { createGraph, components } from "@ailu/graph-sdk";
 *
 * const app = createGraph({ name: "prompt" })
 *   .channel("name", { type: "string", default: "" })
 *   .channel("prompt", { type: "string", default: "" })
 *   .component("build", components.promptBuilder({ template: "Hi {{name}}", into: "prompt" }))
 *   .compile();
 * ```
 */

import { createHash } from "node:crypto";

import type { NodeHandler } from "@ailu/graph-runtime";

/** The component kinds the library knows, matching `ComponentRegistry::kinds()`. */
export type ComponentKind =
  | "promptBuilder"
  | "jsonValidator"
  | "outputParser"
  | "router"
  | "retriever"
  | "reranker"
  | "textCleaner"
  | "documentSplitter"
  | "htmlToText"
  | "csvParser"
  | "documentJoiner"
  | "deduplicator"
  | "truncator"
  | "regexExtractor"
  | "answerBuilder"
  | "fieldMapper"
  | "fieldExtractor"
  // --- wave two: Haystack-gap coverage (all pure, deterministic) ---
  | "bm25Retriever"
  | "keywordRetriever"
  | "sentenceWindowSplitter"
  | "languageDetector"
  | "metadataFilter"
  | "listJoiner"
  | "mergeRanker"
  | "evaluator"
  | "chatMessageBuilder"
  | "conditionalRouter"
  | "documentWriter";

/**
 * The serializable projection of a component node the Rust engine bridge consumes
 * (the Phase C `componentNodes` carrier, camelCase): a component `kind` plus its
 * validated `params` object. Pure data — no closures.
 */
export type RustComponentConfig = {
  kind: ComponentKind;
  params: Record<string, unknown>;
};

/**
 * What a component factory returns: the Phase C carrier (`kind` + `params`) so the
 * node runs natively on Rust, plus an equivalent TS {@link NodeHandler} for the
 * fallback path. {@link GraphBuilder.component} pushes a node from this descriptor.
 */
export type ComponentDescriptor = RustComponentConfig & {
  /** The faithful TS-equivalent handler used when the native addon is absent. */
  handler: NodeHandler;
};

/**
 * What an **integration component** factory returns (the vendor-I/O pattern, e.g.
 * {@link components.httpFetch} / {@link components.webSearch}). Unlike a
 * {@link ComponentDescriptor}, an integration component is **not** a Rust component:
 * it has no `kind`/`params` carrier and is **not** registered in `componentNodes`.
 * It is a plain `NodeHandler` (an injectable closure over an injected I/O impl) added
 * as a regular JS node via {@link import("./builder.js").GraphBuilder.node}; on the
 * Rust engine it runs over the async JS seam (`on_node`) like any other JS handler.
 *
 * ```ts
 * createGraph({ name: "fetch" })
 *   .channel("body", { type: "json", default: null })
 *   .node("get", components.httpFetch({ url: "https://x", into: "body", fetchImpl: fake }));
 * ```
 */
export type IntegrationComponentHandler = NodeHandler;

// --- shared text/value helpers (mirror the Rust `value_to_text`) -------------

/**
 * Coerce a channel value to text the way the Rust `value_to_text` does: strings pass
 * through unquoted, `null`/`undefined` become the empty string, everything else is
 * compact JSON.
 */
const valueToText = (value: unknown): string => {
  if (typeof value === "string") {
    return value;
  }
  if (value === null || value === undefined) {
    return "";
  }
  return JSON.stringify(value);
};

const channelsOf = (state: { channels: unknown }): Record<string, unknown> =>
  (state.channels ?? {}) as Record<string, unknown>;

// --- promptBuilder -----------------------------------------------------------

/** ADR 0044 D3 (ailu#578) capsule-assembly config for {@link components.promptBuilder}.
 * `chunksFrom`/`queryFrom` are required, `discardedFrom` is optional (defaults to none) — a
 * capsule-assembly node needs to be told explicitly which upstream channel holds the query and
 * which channels hold this pipeline's discarded candidates (they can be scattered across several
 * stages — D2's `{into}Discarded` channels are stage-local, not a single sibling of `chunksFrom`). */
export type PromptBuilderCapsuleConfig = {
  into: string;
  chunksFrom: string;
  queryFrom: string;
  discardedFrom?: string[];
};

/** Params for {@link components.promptBuilder}. */
export type PromptBuilderParams = {
  /** Template with `{{var}}` placeholders filled from the channels. */
  template: string;
  /** Channel the rendered string is written into. */
  into: string;
  /** Optional retrieval-proof-capsule assembly (ADR 0044 D3) — captures the exact set that
   * influenced the model, hashed at the ONE node that knows what text was actually templated. */
  capsule?: PromptBuilderCapsuleConfig;
};

/**
 * Render every `{{ name }}` placeholder from the channels (whitespace inside the
 * braces tolerated; unknown placeholders render empty). Mirrors `render_template`.
 */
const renderTemplate = (template: string, channels: Record<string, unknown>): string =>
  template.replace(/\{\{\s*([^}]*?)\s*\}\}/g, (_match, rawName: string) => {
    const name = rawName.trim();
    return name in channels ? valueToText(channels[name]) : "";
  });

const sha256Hex = (input: string): string => createHash("sha256").update(input).digest("hex");

/** Assemble the `RetrievalCapsule` (ADR 0044 D3). Mirrors `build_retrieval_capsule` in Rust. */
const buildRetrievalCapsule = (
  cfg: PromptBuilderCapsuleConfig,
  channels: Record<string, unknown>,
  rendered: string
): unknown => {
  const chunksArr = Array.isArray(channels[cfg.chunksFrom]) ? (channels[cfg.chunksFrom] as unknown[]) : [];
  const asRecord = (item: unknown): Record<string, unknown> =>
    typeof item === "object" && item !== null && !Array.isArray(item) ? (item as Record<string, unknown>) : {};
  const order = chunksArr.map((item) => asRecord(item).id ?? null);
  const chunks = chunksArr.map((item) => {
    const record = asRecord(item);
    return { id: record.id ?? null, provenance: record.provenance ?? [] };
  });

  const discarded = (cfg.discardedFrom ?? []).flatMap((name) =>
    Array.isArray(channels[name]) ? (channels[name] as unknown[]) : []
  );

  const queryText = cfg.queryFrom in channels ? valueToText(channels[cfg.queryFrom]) : "";

  return {
    order,
    chunks,
    discarded,
    renderedHash: sha256Hex(rendered),
    queryHash: sha256Hex(queryText)
  };
};

const promptBuilderHandler =
  (params: PromptBuilderParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const rendered = renderTemplate(params.template, channels);
    const update: Record<string, unknown> = { [params.into]: rendered };
    if (params.capsule !== undefined) {
      update[params.capsule.into] = buildRetrievalCapsule(params.capsule, channels, rendered);
    }
    return update;
  };

// --- jsonValidator -----------------------------------------------------------

/** Params for {@link components.jsonValidator}. */
export type JsonValidatorParams = {
  /** Channel whose value is validated. */
  from: string;
  /** Required object keys to assert present. */
  requiredKeys?: string[];
  /** Expected JSON type (`"object" | "array" | "string" | ...`). */
  expectType?: "string" | "number" | "boolean" | "object" | "array" | "null";
  /** Channel receiving the `boolean` validity flag. */
  okInto: string;
  /** Channel receiving the `string[]` of validation errors. */
  errorsInto: string;
};

/** The JSON type name of a value, matching the Rust `json_type_name` vocabulary. */
const jsonTypeName = (value: unknown): JsonValidatorParams["expectType"] => {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "array";
  }
  switch (typeof value) {
    case "boolean":
      return "boolean";
    case "number":
      return "number";
    case "string":
      return "string";
    case "object":
      return "object";
    default:
      // `undefined`/`function`/`symbol` aren't JSON; treat as null, matching the
      // Rust path where an absent channel reads as `Value::Null`.
      return "null";
  }
};

const jsonValidatorHandler =
  (params: JsonValidatorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const value = params.from in channels ? channels[params.from] : null;
    const errors: string[] = [];

    if (params.expectType !== undefined) {
      const actual = jsonTypeName(value);
      if (actual !== params.expectType) {
        errors.push(`expected type \`${params.expectType}\` but got \`${actual}\``);
      }
    }

    const requiredKeys = params.requiredKeys ?? [];
    if (requiredKeys.length > 0) {
      if (typeof value === "object" && value !== null && !Array.isArray(value)) {
        const obj = value as Record<string, unknown>;
        for (const key of requiredKeys) {
          if (!(key in obj)) {
            errors.push(`missing required key \`${key}\``);
          }
        }
      } else {
        errors.push("expected an object to check required keys");
      }
    }

    return { [params.okInto]: errors.length === 0, [params.errorsInto]: errors };
  };

// --- outputParser ------------------------------------------------------------

/** Params for {@link components.outputParser}. */
export type OutputParserParams = {
  /** Text channel to extract the first JSON value from. */
  from: string;
  /** Channel receiving the parsed value (or `null` when none is found). */
  into: string;
};

/**
 * Find the first balanced JSON object or array in `text` and parse it, skipping over
 * string literals (and escaped quotes) so braces inside strings don't confuse depth.
 * Mirrors the Rust `extract_first_json`. Returns `null` when nothing parses.
 */
const extractFirstJson = (text: string): unknown => {
  const startMatch = /[{[]/.exec(text);
  if (startMatch === null) {
    return null;
  }
  const start = startMatch.index;
  const open = text[start];
  const close = open === "{" ? "}" : "]";

  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === '"') {
        inString = false;
      }
    } else if (ch === '"') {
      inString = true;
    } else if (ch === open) {
      depth += 1;
    } else if (ch === close) {
      depth -= 1;
      if (depth === 0) {
        const candidate = text.slice(start, i + 1);
        try {
          return JSON.parse(candidate) as unknown;
        } catch {
          return null;
        }
      }
    }
  }
  return null;
};

const outputParserHandler =
  (params: OutputParserParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const raw = params.from in channels ? valueToText(channels[params.from]) : "";
    return { [params.into]: extractFirstJson(raw) };
  };

// --- router ------------------------------------------------------------------

/** One routing rule for {@link components.router}. */
export type RouterRule = {
  /** Exact match against the textual form of the `from` value. */
  equals?: string;
  /** Substring match against the textual form of the `from` value. */
  contains?: string;
  /** The route string emitted when this rule matches. */
  route: string;
};

/** Params for {@link components.router}. */
export type RouterParams = {
  /** Channel whose value is matched against the rules. */
  from: string;
  /** Ordered rules; the first match wins. */
  rules: RouterRule[];
  /** Route emitted when no rule matches. */
  defaultRoute: string;
  /** Channel the chosen route string is written into. */
  into: string;
};

/**
 * Whether a rule matches: `equals` checks exact text equality; `contains` checks for
 * a substring; both must hold when both are set; neither set never matches. Mirrors
 * the Rust `rule_matches`.
 */
const ruleMatches = (rule: RouterRule, text: string): boolean => {
  let hadPredicate = false;
  if (rule.equals !== undefined) {
    hadPredicate = true;
    if (text !== rule.equals) {
      return false;
    }
  }
  if (rule.contains !== undefined) {
    hadPredicate = true;
    if (!text.includes(rule.contains)) {
      return false;
    }
  }
  return hadPredicate;
};

const routerHandler =
  (params: RouterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const value = params.from in channels ? channels[params.from] : null;
    const text = valueToText(value);
    const chosen = params.rules.find((rule) => ruleMatches(rule, text))?.route ?? params.defaultRoute;
    return { [params.into]: chosen };
  };

// --- retriever / reranker shared embedding ----------------------------------

/**
 * The deterministic 4-bucket count vector used by `ailu_rag_pipeline`'s mock
 * embedder: bucket `codePoint % 4` is incremented per character. Mirrors the Rust
 * `mock_embed` (which uses `ch as u32`), so iterate by code point.
 */
const mockEmbed = (text: string): number[] => {
  const counts = [0, 0, 0, 0];
  for (const char of text) {
    const idx = (char.codePointAt(0) ?? 0) % counts.length;
    counts[idx] = (counts[idx] ?? 0) + 1;
  }
  return counts;
};

/** Cosine similarity of two equal-length vectors; mirrors `cosine_similarity`. */
const cosineSimilarity = (a: number[], b: number[]): number => {
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let i = 0; i < a.length; i += 1) {
    const av = a[i] ?? 0;
    const bv = b[i] ?? 0;
    dot += av * bv;
    normA += av * av;
    normB += bv * bv;
  }
  if (normA === 0 || normB === 0) {
    return 0;
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB));
};

// --- retriever ---------------------------------------------------------------

/** A candidate document for {@link components.retriever}. */
export type RetrieverDoc = { id: string; content: string };

/** Params for {@link components.retriever}. */
export type RetrieverParams = {
  /** Channel holding the query text (falls back to this literal when the channel is empty). */
  query: string;
  /** Channel receiving the top-`k` `{ id, content, score }` array. */
  into: string;
  /** Number of results to keep (default 4). */
  k?: number;
  /** The corpus to score against. */
  docs: RetrieverDoc[];
};

const retrieverHandler =
  (params: RetrieverParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const channelText = params.query in channels ? valueToText(channels[params.query]) : "";
    const queryText = channelText.length > 0 ? channelText : params.query;
    const queryVec = mockEmbed(queryText);
    const k = params.k ?? 4;

    const scored = params.docs.map((doc, index) => ({
      doc,
      index,
      score: cosineSimilarity(queryVec, mockEmbed(doc.content))
    }));
    // Descending by score; stable so input order breaks ties (matches the Rust
    // stable sort over input order).
    scored.sort((a, b) => (b.score - a.score === 0 ? a.index - b.index : b.score - a.score));

    const results = scored
      .slice(0, k)
      .map(({ doc, score }) => ({ id: doc.id, content: doc.content, score }));
    return { [params.into]: results };
  };

// --- reranker ----------------------------------------------------------------

/** Params for {@link components.reranker}. */
export type RerankerParams = {
  /** Channel holding the retrieval-result array to reorder. */
  from: string;
  /** Channel receiving the reordered array. */
  into: string;
  /** Optional channel holding the query text the cross-encoder (`AILU_RERANK_ENDPOINT`)
   * re-scores against. Without an endpoint, items keep their upstream order (by `score`). */
  query?: string;
};

const rerankerHandler =
  (params: RerankerParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const fromValue = params.from in channels ? channels[params.from] : undefined;
    const items: Record<string, unknown>[] = Array.isArray(fromValue)
      ? (fromValue as Record<string, unknown>[])
      : [];

    // Optional query text for embedding-based rescoring.
    let queryVec: number[] | undefined;
    if (params.query !== undefined && params.query in channels) {
      const text = valueToText(channels[params.query]);
      if (text.length > 0) {
        queryVec = mockEmbed(text);
      }
    }

    const scored = items.map((item, index) => {
      let score: number;
      if (queryVec !== undefined) {
        const content = valueToText((item as { content?: unknown }).content);
        score = cosineSimilarity(queryVec, mockEmbed(content));
      } else {
        const existing = (item as { score?: unknown }).score;
        score = typeof existing === "number" ? existing : 0;
      }
      return { item, index, score };
    });
    scored.sort((a, b) => (b.score - a.score === 0 ? a.index - b.index : b.score - a.score));

    const reordered = scored.map(({ item, score }) =>
      typeof item === "object" && item !== null && !Array.isArray(item)
        ? { ...item, score }
        : item
    );
    return { [params.into]: reordered };
  };

// --- textCleaner -------------------------------------------------------------

/** Params for {@link components.textCleaner}. */
export type TextCleanerParams = {
  /** Channel whose text is normalised. */
  from: string;
  /** Channel receiving the cleaned text. */
  into: string;
  /** Lowercase the text. Defaults to `false`. */
  lowercase?: boolean;
  /** Strip `<…>` HTML tags. Defaults to `false`. */
  stripHtml?: boolean;
  /** Collapse runs of whitespace to a single space. Defaults to `false`. */
  collapseWhitespace?: boolean;
  /** Trim leading/trailing whitespace. Defaults to `false`. */
  trim?: boolean;
};

/**
 * Remove anything between `<` and the next `>` (a simple, deterministic tag
 * stripper — it does not parse nested or malformed markup beyond this rule).
 * Mirrors the Rust `strip_html_tags`.
 */
const stripHtmlTags = (text: string): string => {
  let out = "";
  let inTag = false;
  for (const ch of text) {
    if (ch === "<") {
      inTag = true;
    } else if (ch === ">") {
      inTag = false;
    } else if (!inTag) {
      out += ch;
    }
  }
  return out;
};

/** Collapse every run of whitespace into a single space. Mirrors `collapse_ws`. */
const collapseWs = (text: string): string => text.split(/\s+/).filter((s) => s.length > 0).join(" ");

const textCleanerHandler =
  (params: TextCleanerParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    let text = params.from in channels ? valueToText(channels[params.from]) : "";
    // Fixed order so the result is deterministic regardless of param order:
    // stripHtml -> lowercase -> collapseWhitespace -> trim.
    if (params.stripHtml === true) {
      text = stripHtmlTags(text);
    }
    if (params.lowercase === true) {
      text = text.toLowerCase();
    }
    if (params.collapseWhitespace === true) {
      text = collapseWs(text);
    }
    if (params.trim === true) {
      text = text.trim();
    }
    return { [params.into]: text };
  };

// --- documentSplitter --------------------------------------------------------

/** Params for {@link components.documentSplitter}. */
export type DocumentSplitterParams = {
  /** Channel holding the text to split. */
  from: string;
  /** Channel receiving the `string[]` of chunks. */
  into: string;
  /** Split unit: `"chars"` sliding windows or greedy `"sentences"` packing. */
  by: "chars" | "sentences";
  /** Window size in characters (`by:"chars"`) or sentences (`by:"sentences"`). Must be > 0. */
  size: number;
  /** Overlap repeated at the start of each next chunk. Must be smaller than `size`. Defaults to 0. */
  overlap?: number;
};

/** Slice `text` into windows of `size` chars, stepping by `size - overlap`. Mirrors `split_by_chars`. */
const splitByChars = (text: string, size: number, overlap: number): string[] => {
  const chars = [...text];
  if (chars.length === 0) {
    return [];
  }
  const step = size - overlap;
  const chunks: string[] = [];
  let start = 0;
  while (start < chars.length) {
    const end = Math.min(start + size, chars.length);
    chunks.push(chars.slice(start, end).join(""));
    if (end === chars.length) {
      break;
    }
    start += step;
  }
  return chunks;
};

/**
 * Split text into trimmed, non-empty sentences on `.`/`!`/`?` terminators,
 * keeping the terminator attached to its sentence. Mirrors `segment_sentences`.
 */
const segmentSentences = (text: string): string[] => {
  const sentences: string[] = [];
  let current = "";
  for (const ch of text) {
    current += ch;
    if (ch === "." || ch === "!" || ch === "?") {
      const trimmed = current.trim();
      if (trimmed.length > 0) {
        sentences.push(trimmed);
      }
      current = "";
    }
  }
  const trimmed = current.trim();
  if (trimmed.length > 0) {
    sentences.push(trimmed);
  }
  return sentences;
};

/**
 * Segment `text` into sentences, then pack `size` sentences per chunk with
 * `overlap` sentences repeated at the start of each subsequent chunk. Mirrors
 * `split_by_sentences`.
 */
const splitBySentences = (text: string, size: number, overlap: number): string[] => {
  const sentences = segmentSentences(text);
  if (sentences.length === 0) {
    return [];
  }
  const step = size - overlap;
  const chunks: string[] = [];
  let start = 0;
  while (start < sentences.length) {
    const end = Math.min(start + size, sentences.length);
    chunks.push(sentences.slice(start, end).join(" "));
    if (end === sentences.length) {
      break;
    }
    start += step;
  }
  return chunks;
};

const documentSplitterHandler =
  (params: DocumentSplitterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const overlap = params.overlap ?? 0;
    const chunks =
      params.by === "chars"
        ? splitByChars(text, params.size, overlap)
        : splitBySentences(text, params.size, overlap);
    return { [params.into]: chunks };
  };

// --- htmlToText --------------------------------------------------------------

/** Params for {@link components.htmlToText}. */
export type HtmlToTextParams = {
  /** Channel holding the HTML text. */
  from: string;
  /** Channel receiving the tag-stripped, entity-decoded text. */
  into: string;
};

/**
 * Decode the common named HTML entities. `&amp;` is decoded last so an input
 * like `&amp;lt;` round-trips to `&lt;` rather than being double-decoded.
 * Mirrors the Rust `decode_entities`.
 */
const decodeEntities = (text: string): string =>
  text
    .split("&lt;")
    .join("<")
    .split("&gt;")
    .join(">")
    .split("&quot;")
    .join('"')
    .split("&amp;")
    .join("&");

const htmlToTextHandler =
  (params: HtmlToTextParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const raw = params.from in channels ? valueToText(channels[params.from]) : "";
    return { [params.into]: decodeEntities(stripHtmlTags(raw)) };
  };

// --- csvParser ---------------------------------------------------------------

/** Params for {@link components.csvParser}. */
export type CsvParserParams = {
  /** Channel holding the CSV text. */
  from: string;
  /** Channel receiving the parsed rows array. */
  into: string;
  /** Single-character cell delimiter. Defaults to `","`. */
  delimiter?: string;
  /** When `true` (default) the first row supplies object keys; otherwise rows are arrays. */
  header?: boolean;
};

const csvParserHandler =
  (params: CsvParserParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const delimiter = params.delimiter ?? ",";
    const header = params.header ?? true;

    // Simple line/char splitter: rows on `\n`, cells on the delimiter (no quoted
    // cells / embedded newlines), skipping blank lines. Mirrors the Rust parser.
    const rows = text
      .split("\n")
      .filter((line) => line.trim().length > 0)
      .map((line) => line.split(delimiter).map((cell) => cell.trim()));

    let parsed: unknown[];
    if (header) {
      const headers = rows[0] ?? [];
      parsed = rows.slice(1).map((cells) => {
        const obj: Record<string, string> = {};
        headers.forEach((name, i) => {
          obj[name] = cells[i] ?? "";
        });
        return obj;
      });
    } else {
      parsed = rows.map((cells) => [...cells]);
    }
    return { [params.into]: parsed };
  };

// --- shared dedupe -----------------------------------------------------------

/**
 * De-duplicate `items`, keeping the first occurrence and preserving order. The
 * dedupe identity is: if `key` is set and the item is an object with that field,
 * the text form of that field; otherwise the item's value text. Mirrors the Rust
 * `dedupe_array` (strings dedupe by their unquoted text).
 */
const dedupeArray = (items: unknown[], key?: string): unknown[] => {
  const seen = new Set<string>();
  const out: unknown[] = [];
  for (const item of items) {
    let identity: string;
    if (
      key !== undefined &&
      typeof item === "object" &&
      item !== null &&
      !Array.isArray(item) &&
      key in (item as Record<string, unknown>)
    ) {
      identity = valueToText((item as Record<string, unknown>)[key]);
    } else {
      identity = valueToText(item);
    }
    if (!seen.has(identity)) {
      seen.add(identity);
      out.push(item);
    }
  }
  return out;
};

// --- documentJoiner ----------------------------------------------------------

/** Params for {@link components.documentJoiner}. */
export type DocumentJoinerParams = {
  /** Channels whose array values are concatenated in order. */
  fromChannels: string[];
  /** Channel receiving the merged array. */
  into: string;
  /** Optional object field to de-duplicate the merged items by. */
  dedupeBy?: string;
};

const documentJoinerHandler =
  (params: DocumentJoinerParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    let merged: unknown[] = [];
    for (const name of params.fromChannels) {
      const value = name in channels ? channels[name] : undefined;
      if (Array.isArray(value)) {
        merged = merged.concat(value);
      }
    }
    if (params.dedupeBy !== undefined) {
      merged = dedupeArray(merged, params.dedupeBy);
    }
    return { [params.into]: merged };
  };

// --- deduplicator ------------------------------------------------------------

/** Params for {@link components.deduplicator}. */
export type DeduplicatorParams = {
  /** Channel holding the array to de-duplicate. */
  from: string;
  /** Channel receiving the de-duplicated array. */
  into: string;
  /** Optional object field to compare items by (else whole-value compare). */
  key?: string;
};

const deduplicatorHandler =
  (params: DeduplicatorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const value = params.from in channels ? channels[params.from] : undefined;
    const items: unknown[] = Array.isArray(value) ? value : [];
    return { [params.into]: dedupeArray(items, params.key) };
  };

// --- truncator ---------------------------------------------------------------

/** Params for {@link components.truncator}. */
export type TruncatorParams = {
  /** Channel holding the text to truncate. */
  from: string;
  /** Channel receiving the (possibly truncated) text. */
  into: string;
  /** Maximum character length (the ellipsis counts against this budget). */
  maxChars: number;
  /** Suffix appended when truncated. Defaults to `"…"`. */
  ellipsis?: string;
};

const truncatorHandler =
  (params: TruncatorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const ellipsis = params.ellipsis ?? "…";
    const chars = [...text];
    let truncated: string;
    if (chars.length <= params.maxChars) {
      truncated = text;
    } else {
      const ellipsisLen = [...ellipsis].length;
      if (ellipsisLen >= params.maxChars) {
        truncated = chars.slice(0, params.maxChars).join("");
      } else {
        const keep = params.maxChars - ellipsisLen;
        truncated = chars.slice(0, keep).join("") + ellipsis;
      }
    }
    return { [params.into]: truncated };
  };

// --- regexExtractor ----------------------------------------------------------

/** Params for {@link components.regexExtractor}. */
export type RegexExtractorParams = {
  /** Channel holding the text to match against. */
  from: string;
  /** Channel receiving the match (or matches when `all`). */
  into: string;
  /**
   * Literal-substring pattern with optional leading `^` (start) and trailing `$`
   * (end) anchors. No character classes / quantifiers / capture groups.
   */
  pattern: string;
  /** Accepted for forward-compat; only `0` (the whole match) is supported. Defaults to 0. */
  group?: number;
  /** When `true`, return every non-overlapping occurrence as an array. Defaults to `false`. */
  all?: boolean;
};

/**
 * Find literal `pattern` occurrences in `text`, honouring leading `^` (start
 * anchor) and trailing `$` (end anchor). Mirrors the Rust `literal_matches`.
 */
const literalMatches = (text: string, pattern: string, all: boolean): string[] => {
  const anchoredStart = pattern.startsWith("^");
  const anchoredEnd = pattern.endsWith("$");
  const startTrim = anchoredStart ? 1 : 0;
  const endTrim = anchoredEnd ? 1 : 0;
  if (startTrim + endTrim >= pattern.length) {
    // Pattern is only anchors with no literal body.
    return [];
  }
  const literal = pattern.slice(startTrim, pattern.length - endTrim);

  if (anchoredStart && anchoredEnd) {
    return text === literal ? [literal] : [];
  }
  if (anchoredStart) {
    return text.startsWith(literal) ? [literal] : [];
  }
  if (anchoredEnd) {
    return text.endsWith(literal) ? [literal] : [];
  }

  // Unanchored: scan for non-overlapping occurrences.
  const out: string[] = [];
  let rest = text;
  let pos = rest.indexOf(literal);
  while (pos !== -1) {
    out.push(literal);
    if (!all) {
      break;
    }
    rest = rest.slice(pos + literal.length);
    pos = rest.indexOf(literal);
  }
  return out;
};

const regexExtractorHandler =
  (params: RegexExtractorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const group = params.group ?? 0;
    const all = params.all ?? false;
    // Only the whole-match group (0) is supported by the literal matcher.
    const matches = group === 0 ? literalMatches(text, params.pattern, all) : [];
    const value = all ? matches : (matches[0] ?? null);
    return { [params.into]: value };
  };

// --- answerBuilder -----------------------------------------------------------

/** Params for {@link components.answerBuilder}. */
export type AnswerBuilderParams = {
  /** Channel supplying the core answer text. */
  from: string;
  /** Channel receiving the assembled answer. */
  into: string;
  /** Optional channel holding a retrieval-result array rendered as numbered citations. */
  contextFrom?: string;
  /** Optional `{{answer}}`/`{{citations}}` template controlling the layout. */
  template?: string;
};

/**
 * Render a retrieval-result array into a numbered citation block: one
 * `"[n] <id>: <content>"` line per item (id omitted when absent). Mirrors the
 * Rust `render_citations`.
 */
const renderCitations = (items: unknown[]): string => {
  const lines: string[] = [];
  items.forEach((item, index) => {
    const n = index + 1;
    const obj = typeof item === "object" && item !== null ? (item as Record<string, unknown>) : {};
    const idValue = obj.id;
    const id = typeof idValue === "string" ? idValue : undefined;
    const content = "content" in obj ? valueToText(obj.content) : valueToText(item);
    lines.push(id !== undefined ? `[${n}] ${id}: ${content}` : `[${n}] ${content}`);
  });
  return lines.join("\n");
};

const answerBuilderHandler =
  (params: AnswerBuilderParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const answer = params.from in channels ? valueToText(channels[params.from]) : "";

    let citations = "";
    if (params.contextFrom !== undefined) {
      const value = params.contextFrom in channels ? channels[params.contextFrom] : undefined;
      if (Array.isArray(value)) {
        citations = renderCitations(value);
      }
    }

    let result: string;
    if (params.template !== undefined) {
      result = renderTemplate(params.template, { answer, citations });
    } else if (citations.length === 0) {
      result = answer;
    } else {
      result = `${answer}\n\nSources:\n${citations}`;
    }
    return { [params.into]: result };
  };

// --- fieldMapper -------------------------------------------------------------

/** Params for {@link components.fieldMapper}. */
export type FieldMapperParams = {
  /** Channel holding the source object. */
  from: string;
  /** Channel receiving the remapped object. */
  into: string;
  /** `{ outKey: inKeyPath }` map; `inKeyPath` is a dotted path into the source. */
  mapping: Record<string, string>;
};

/**
 * Resolve a dotted path (`"a.b.c"`) into a JSON value, descending through
 * objects. Returns `undefined` if any segment is missing or a non-object is hit.
 * Mirrors the Rust `resolve_path`.
 */
const resolvePath = (value: unknown, path: string): unknown => {
  let current: unknown = value;
  for (const segment of path.split(".")) {
    if (typeof current === "object" && current !== null && !Array.isArray(current) && segment in (current as Record<string, unknown>)) {
      current = (current as Record<string, unknown>)[segment];
    } else {
      return undefined;
    }
  }
  return current;
};

const fieldMapperHandler =
  (params: FieldMapperParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const source = params.from in channels ? channels[params.from] : null;
    const obj: Record<string, unknown> = {};
    for (const [outKey, path] of Object.entries(params.mapping)) {
      const resolved = resolvePath(source, path);
      obj[outKey] = resolved === undefined ? null : resolved;
    }
    return { [params.into]: obj };
  };

// --- fieldExtractor ----------------------------------------------------------

/** Params for {@link components.fieldExtractor}. */
export type FieldExtractorParams = {
  /** Channel holding the source value. */
  from: string;
  /** Channel receiving the extracted scalar. */
  into: string;
  /** Optional dotted path descended into the `from` value (else the whole value). */
  path?: string;
  /**
   * When `true`, if the resulting value is a string containing a `"final:"` marker,
   * return only the text after the **last** `"final:"` (trimmed) — reduces an
   * agent reasoning trace to its final answer. Non-strings pass through. Defaults to `false`.
   */
  finalOnly?: boolean;
};

/**
 * Return only the text after the **last** `"final:"` marker (trimmed); if the
 * marker is absent, return the text unchanged. Mirrors the Rust `extract_final_answer`.
 */
const extractFinalAnswer = (text: string): string => {
  const pos = text.lastIndexOf("final:");
  return pos === -1 ? text : text.slice(pos + "final:".length).trim();
};

const fieldExtractorHandler =
  (params: FieldExtractorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const source = params.from in channels ? channels[params.from] : null;
    let value: unknown;
    if (params.path !== undefined) {
      const resolved = resolvePath(source, params.path);
      value = resolved === undefined ? null : resolved;
    } else {
      value = source;
    }
    if (params.finalOnly === true && typeof value === "string") {
      value = extractFinalAnswer(value);
    }
    return { [params.into]: value };
  };

// --- shared lexical helpers (mirror the Rust `tokenize` / `query_text`) -------

/**
 * Tokenize text into lowercase alphanumeric word tokens, dropping punctuation
 * and empty runs. Mirrors the Rust `tokenize` (split on non-alphanumeric).
 */
const tokenize = (text: string): string[] =>
  text
    .split(/[^\p{L}\p{N}]+/u)
    .filter((w) => w.length > 0)
    .map((w) => w.toLowerCase());

/**
 * Read the query text for a retriever: prefer the `query` channel's text,
 * falling back to the literal `query` param value when the channel is empty.
 * Mirrors the Rust `query_text`.
 */
const queryText = (channels: Record<string, unknown>, queryParam: string): string => {
  const channelText = queryParam in channels ? valueToText(channels[queryParam]) : "";
  return channelText.length > 0 ? channelText : queryParam;
};

/** Read an array channel, returning `[]` for a missing / non-array channel. */
const arrayChannel = (channels: Record<string, unknown>, name: string): unknown[] => {
  const value = name in channels ? channels[name] : undefined;
  return Array.isArray(value) ? value : [];
};

// --- bm25Retriever -----------------------------------------------------------

/** A candidate document for the lexical retrievers. */
export type LexicalDoc = { id: string; content: string };

/** One stage's score in a candidate's retrieval lineage (ADR 0044, ailu-engine, issue #578) —
 * additive: each retrieval/fusion/rerank stage APPENDS its own step instead of overwriting the
 * top-level `score` it inherited, so a candidate's full history (which stage scored it what, with
 * which algorithm/version) survives all the way to the prompt that gets templated from it. The
 * top-level `score` field on a result still mirrors the LAST step's score, unchanged, for any
 * caller that only cares about the current ranking. */
export type ProvenanceStep = {
  stage: string;
  algorithm: string;
  algorithmVersion: string | null;
  score: number;
};

/** `item.provenance` when present and well-formed, else `[]` — every stage reads whatever lineage
 * an upstream stage already attached (or none, if it's the first stage a candidate passes
 * through) before appending its own step. */
const priorProvenance = (item: unknown): ProvenanceStep[] =>
  typeof item === "object" && item !== null && !Array.isArray(item) && Array.isArray((item as { provenance?: unknown }).provenance)
    ? ((item as { provenance: unknown[] }).provenance as ProvenanceStep[])
    : [];

/** Params for {@link components.bm25Retriever}. */
export type Bm25RetrieverParams = {
  /** Channel holding the query text (falls back to this literal when empty). */
  query: string;
  /** Channel receiving the top-`k` `{ id, content, score }` array. */
  into: string;
  /** Number of results to keep (default 4). */
  k?: number;
  /** The corpus to rank. */
  docs: LexicalDoc[];
  /** BM25 term-frequency saturation. Defaults to 1.2. */
  k1?: number;
  /** BM25 length-normalization. Defaults to 0.75. */
  b?: number;
};

const bm25RetrieverHandler =
  (params: Bm25RetrieverParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const qTokens = tokenize(queryText(channels, params.query));
    const k = params.k ?? 4;
    const k1 = params.k1 ?? 1.2;
    const b = params.b ?? 0.75;

    const docTokens = params.docs.map((d) => tokenize(d.content));
    const docCount = Math.max(docTokens.length, 1);
    const avgLen =
      docTokens.length === 0
        ? 0
        : docTokens.reduce((sum, t) => sum + t.length, 0) / docTokens.length;
    const df = new Map<string, number>();
    for (const tokens of docTokens) {
      for (const tok of new Set(tokens)) {
        df.set(tok, (df.get(tok) ?? 0) + 1);
      }
    }

    const scored = params.docs.map((doc, index) => {
      const tokens = docTokens[index] ?? [];
      const len = tokens.length;
      let score = 0;
      for (const q of qTokens) {
        const f = tokens.filter((t) => t === q).length;
        if (f === 0) {
          continue;
        }
        const nQ = df.get(q) ?? 0;
        const idf = Math.log((docCount - nQ + 0.5) / (nQ + 0.5) + 1.0);
        const denom = f + k1 * (1 - b + b * (len / Math.max(avgLen, 1)));
        score += (idf * (f * (k1 + 1))) / denom;
      }
      return { doc, index, score };
    });
    scored.sort((a, b2) => (b2.score - a.score === 0 ? a.index - b2.index : b2.score - a.score));

    // ADR 0044 D2 (ailu#578): everything past `k` used to be dropped with zero trace — split
    // instead of slicing-and-discarding, so the loser half survives on `{into}Discarded`.
    const discardedAt = String(Date.now());
    const provenanceStep = (score: number): ProvenanceStep => ({
      stage: "bm25",
      algorithm: "bm25",
      algorithmVersion: `k1=${k1},b=${b}`,
      score
    });
    const kept = scored.slice(0, k).map(({ doc, score }) => ({
      id: doc.id,
      content: doc.content,
      score,
      provenance: [provenanceStep(score)] satisfies ProvenanceStep[]
    }));
    const discarded = scored.slice(k).map(({ doc, score }) => ({
      id: doc.id,
      content: doc.content,
      score,
      provenance: [provenanceStep(score)] satisfies ProvenanceStep[],
      discardedAt,
      reason: "bm25_rank_below_k"
    }));
    return { [params.into]: kept, [`${params.into}Discarded`]: discarded };
  };

// --- keywordRetriever --------------------------------------------------------

/** Params for {@link components.keywordRetriever}. */
export type KeywordRetrieverParams = {
  /** Channel holding the query text (falls back to this literal when empty). */
  query: string;
  /** Channel receiving the top-`k` `{ id, content, score }` array. */
  into: string;
  /** Number of results to keep (default 4). */
  k?: number;
  /** The corpus to rank. */
  docs: LexicalDoc[];
};

const keywordRetrieverHandler =
  (params: KeywordRetrieverParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const qTerms = [...new Set(tokenize(queryText(channels, params.query)))];
    const denom = Math.max(qTerms.length, 1);
    const k = params.k ?? 4;

    const docSets = params.docs.map((d) => new Set(tokenize(d.content)));
    const scored = params.docs.map((doc, index) => {
      const set = docSets[index] ?? new Set<string>();
      const matched = qTerms.filter((t) => set.has(t)).length;
      return { doc, index, score: matched / denom };
    });
    scored.sort((a, b) => (b.score - a.score === 0 ? a.index - b.index : b.score - a.score));

    const results = scored
      .slice(0, k)
      .map(({ doc, score }) => ({ id: doc.id, content: doc.content, score }));
    return { [params.into]: results };
  };

// --- sentenceWindowSplitter --------------------------------------------------

/** Params for {@link components.sentenceWindowSplitter}. */
export type SentenceWindowSplitterParams = {
  /** Channel holding the text to split. */
  from: string;
  /** Channel receiving the `string[]` of overlapping sentence windows. */
  into: string;
  /** Sentences per window. Defaults to 3. */
  windowSize?: number;
  /** Sentences advanced between windows (`1 <= stride <= windowSize`). Defaults to 1. */
  stride?: number;
};

const sentenceWindowSplitterHandler =
  (params: SentenceWindowSplitterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const windowSize = params.windowSize ?? 3;
    const stride = params.stride ?? 1;
    const sentences = segmentSentences(text);
    const windows: string[] = [];
    if (sentences.length > 0) {
      let start = 0;
      for (;;) {
        const end = Math.min(start + windowSize, sentences.length);
        windows.push(sentences.slice(start, end).join(" "));
        if (end === sentences.length) {
          break;
        }
        start += stride;
      }
    }
    return { [params.into]: windows };
  };

// --- languageDetector --------------------------------------------------------

/** Params for {@link components.languageDetector}. */
export type LanguageDetectorParams = {
  /** Channel holding the text to classify. */
  from: string;
  /** Channel receiving the detected language code (`"en" | "fr" | ... | "und"`). */
  into: string;
  /** Optional channel receiving the winning language's share of hits in `[0, 1]`. */
  confidenceInto?: string;
};

/** Fixed language order doubles as the deterministic tie-break order. Mirrors the Rust table. */
const LANGUAGE_STOPWORDS: ReadonlyArray<readonly [string, readonly string[]]> = [
  ["en", ["the", "and", "is", "of", "to", "in", "that", "it"]],
  ["fr", ["le", "la", "les", "et", "est", "un", "une", "des"]],
  ["es", ["el", "la", "los", "y", "es", "un", "una", "de"]],
  ["de", ["der", "die", "das", "und", "ist", "ein", "eine", "nicht"]],
  ["it", ["il", "la", "che", "di", "e", "un", "una", "per"]]
];

const languageDetectorHandler =
  (params: LanguageDetectorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const text = params.from in channels ? valueToText(channels[params.from]) : "";
    const tokens = tokenize(text);

    const scores = LANGUAGE_STOPWORDS.map(
      ([, stops]) => tokens.filter((t) => stops.includes(t)).length
    );
    const total = scores.reduce((sum, n) => sum + n, 0);

    let bestIndex = 0;
    let best = scores[0] ?? 0;
    for (let i = 1; i < scores.length; i += 1) {
      const s = scores[i] ?? 0;
      // Strictly greater keeps the earliest language on ties (matches the Rust order).
      if (s > best) {
        best = s;
        bestIndex = i;
      }
    }
    const detected = best === 0 ? "und" : (LANGUAGE_STOPWORDS[bestIndex]?.[0] ?? "und");

    const update: Record<string, unknown> = { [params.into]: detected };
    if (params.confidenceInto !== undefined) {
      update[params.confidenceInto] = total === 0 ? 0 : best / total;
    }
    return update;
  };

// --- metadataFilter / conditionalRouter shared predicate ---------------------

/** A predicate operator shared by {@link components.metadataFilter} and {@link components.conditionalRouter}. */
export type PredicateOp =
  | "equals"
  | "notEquals"
  | "contains"
  | "exists"
  | "absent"
  | "gt"
  | "gte"
  | "lt"
  | "lte";

/**
 * Evaluate a single field/op/value predicate against a JSON root, resolving
 * `field` as a dotted path. Mirrors the Rust `predicate_holds`.
 */
const predicateHolds = (root: unknown, field: string, op: PredicateOp, value: unknown): boolean => {
  const resolved = resolvePath(root, field);
  switch (op) {
    case "exists":
      return resolved !== undefined;
    case "absent":
      return resolved === undefined;
    case "equals":
      return resolved !== undefined && valueToText(resolved) === valueToText(value);
    case "notEquals":
      return resolved === undefined || valueToText(resolved) !== valueToText(value);
    case "contains":
      return resolved !== undefined && valueToText(resolved).includes(valueToText(value));
    default: {
      const n = typeof resolved === "number" ? resolved : undefined;
      const target = typeof value === "number" ? value : undefined;
      if (n === undefined || target === undefined) {
        return false;
      }
      switch (op) {
        case "gt":
          return n > target;
        case "gte":
          return n >= target;
        case "lt":
          return n < target;
        case "lte":
          return n <= target;
        default:
          return false;
      }
    }
  }
};

/** Params for {@link components.metadataFilter}. */
export type MetadataFilterParams = {
  /** Channel holding the array to filter. */
  from: string;
  /** Channel receiving the filtered array. */
  into: string;
  /** Dotted path into each item compared by the predicate. */
  field: string;
  /** The predicate operator. */
  op: PredicateOp;
  /** The comparison value (required except for `exists`/`absent`). */
  value?: unknown;
};

const metadataFilterHandler =
  (params: MetadataFilterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const items = arrayChannel(channels, params.from);
    const kept = items.filter((item) => predicateHolds(item, params.field, params.op, params.value));
    return { [params.into]: kept };
  };

// --- listJoiner --------------------------------------------------------------

/** Params for {@link components.listJoiner}. */
export type ListJoinerParams = {
  /** Channels whose array values are combined. */
  fromChannels: string[];
  /** Channel receiving the combined array. */
  into: string;
  /** Combine mode: `"concat"` (default), `"union"` (dedupe), or `"interleave"`. */
  mode?: "concat" | "union" | "interleave";
};

const listJoinerHandler =
  (params: ListJoinerParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const lists = params.fromChannels.map((name) => arrayChannel(channels, name));
    const mode = params.mode ?? "concat";

    let merged: unknown[];
    if (mode === "interleave") {
      merged = [];
      const max = lists.reduce((m, list) => Math.max(m, list.length), 0);
      for (let i = 0; i < max; i += 1) {
        for (const list of lists) {
          if (i < list.length) {
            merged.push(list[i]);
          }
        }
      }
    } else {
      merged = lists.flat();
      if (mode === "union") {
        merged = dedupeArray(merged);
      }
    }
    return { [params.into]: merged };
  };

// --- mergeRanker -------------------------------------------------------------

/** Params for {@link components.mergeRanker}. */
export type MergeRankerParams = {
  /** Channels each holding a retrieval-result array to fuse. */
  fromChannels: string[];
  /** Channel receiving the fused `{ id, content, score }` array. */
  into: string;
  /** Object field identifying items across lists. Defaults to `"id"`. */
  idKey?: string;
  /** Keep only the top-`k` fused results (default: keep all). */
  k?: number;
  /** Reciprocal Rank Fusion constant. Defaults to 60. */
  rrfK?: number;
};

const mergeRankerHandler =
  (params: MergeRankerParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const idKey = params.idKey ?? "id";
    const rrfK = params.rrfK ?? 60;

    const scores = new Map<string, number>();
    const representative = new Map<string, unknown>();
    const firstSeen = new Map<string, number>();
    // ADR 0044: a candidate seen in MULTIPLE `fromChannels` (e.g. both the vector and lexical leg)
    // must keep every leg's provenance, not just whichever channel happened to be processed first
    // (the pre-ADR-0044 `representative` map silently dropped every other leg's lineage).
    const provenanceById = new Map<string, ProvenanceStep[]>();
    let counter = 0;

    for (const name of params.fromChannels) {
      const items = arrayChannel(channels, name);
      items.forEach((item, rank) => {
        const idValue =
          typeof item === "object" && item !== null && !Array.isArray(item)
            ? (item as Record<string, unknown>)[idKey]
            : undefined;
        const id = idValue !== undefined ? valueToText(idValue) : valueToText(item);
        scores.set(id, (scores.get(id) ?? 0) + 1 / (rrfK + rank));
        if (!representative.has(id)) {
          representative.set(id, item);
        }
        if (!firstSeen.has(id)) {
          firstSeen.set(id, counter);
          counter += 1;
        }
        const existing = provenanceById.get(id) ?? [];
        provenanceById.set(id, [...existing, ...priorProvenance(item)]);
      });
    }

    const merged = [...scores.entries()]
      .map(([id, score]) => ({ id, score, order: firstSeen.get(id) ?? 0 }))
      .sort((a, b) => (b.score - a.score === 0 ? a.order - b.order : b.score - a.score));
    // ADR 0044 D2 (ailu#578): RRF losers past `k` used to be dropped with zero trace — split
    // instead of slicing, so they survive on `{into}Discarded`.
    const discardedAt = String(Date.now());
    const splitAt = params.k === undefined ? merged.length : params.k;
    const kept = merged.slice(0, splitAt);
    const discarded = merged.slice(splitAt);

    const buildItem = (id: string, score: number, extra: boolean) => {
      const item = representative.get(id);
      const step: ProvenanceStep = { stage: "rrf", algorithm: "rrf", algorithmVersion: `k=${rrfK}`, score };
      const provenance = [...(provenanceById.get(id) ?? []), step];
      if (typeof item !== "object" || item === null || Array.isArray(item)) return item;
      return extra
        ? { ...item, score, provenance, discardedAt, reason: "rrf_rank_below_k" }
        : { ...item, score, provenance };
    };
    const results = kept.map(({ id, score }) => buildItem(id, score, false));
    const discardedResults = discarded.map(({ id, score }) => buildItem(id, score, true));
    return { [params.into]: results, [`${params.into}Discarded`]: discardedResults };
  };

// --- evaluator ---------------------------------------------------------------

/** Params for {@link components.evaluator}. */
export type EvaluatorParams = {
  /** Channel holding the expected/reference text. */
  expectedFrom: string;
  /** Channel holding the actual/candidate text. */
  actualFrom: string;
  /** Channel receiving the numeric score in `[0, 1]`. */
  into: string;
  /** Scoring metric. Defaults to `"tokenF1"`. */
  metric?: "tokenF1" | "overlap" | "exact";
  /** Optional channel receiving a boolean `score >= threshold`. */
  passInto?: string;
  /** Pass threshold for `passInto`. Defaults to 0.5. */
  threshold?: number;
};

/**
 * Token-overlap F1 of two token multisets. Two empty inputs score `1.0`; one
 * empty side scores `0.0`. Mirrors the Rust `token_f1`.
 */
const tokenF1 = (expected: string[], actual: string[]): number => {
  if (expected.length === 0 && actual.length === 0) {
    return 1;
  }
  if (expected.length === 0 || actual.length === 0) {
    return 0;
  }
  const expectedCounts = new Map<string, number>();
  for (const t of expected) {
    expectedCounts.set(t, (expectedCounts.get(t) ?? 0) + 1);
  }
  let matched = 0;
  for (const t of actual) {
    const count = expectedCounts.get(t) ?? 0;
    if (count > 0) {
      expectedCounts.set(t, count - 1);
      matched += 1;
    }
  }
  if (matched === 0) {
    return 0;
  }
  const precision = matched / actual.length;
  const recall = matched / expected.length;
  return (2 * precision * recall) / (precision + recall);
};

const evaluatorHandler =
  (params: EvaluatorParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const expected = params.expectedFrom in channels ? valueToText(channels[params.expectedFrom]) : "";
    const actual = params.actualFrom in channels ? valueToText(channels[params.actualFrom]) : "";
    const metric = params.metric ?? "tokenF1";

    let score: number;
    if (metric === "exact") {
      score = expected.trim() === actual.trim() ? 1 : 0;
    } else if (metric === "overlap") {
      const e = new Set(tokenize(expected));
      const a = new Set(tokenize(actual));
      if (e.size === 0 && a.size === 0) {
        score = 1;
      } else {
        const inter = [...e].filter((t) => a.has(t)).length;
        const union = new Set([...e, ...a]).size;
        score = union === 0 ? 0 : inter / union;
      }
    } else {
      score = tokenF1(tokenize(expected), tokenize(actual));
    }

    const update: Record<string, unknown> = { [params.into]: score };
    if (params.passInto !== undefined) {
      update[params.passInto] = score >= (params.threshold ?? 0.5);
    }
    return update;
  };

// --- chatMessageBuilder ------------------------------------------------------

/** One message spec for {@link components.chatMessageBuilder}. */
export type ChatMessageSpec = {
  /** The message role. */
  role: "system" | "user" | "assistant";
  /** A literal body, rendered through the `{{var}}` template engine. */
  content?: string;
  /** A channel name whose value supplies the body verbatim. */
  contentFrom?: string;
};

/** Params for {@link components.chatMessageBuilder}. */
export type ChatMessageBuilderParams = {
  /** Channel receiving the `[{ role, content }]` array. */
  into: string;
  /** The ordered message specs. */
  messages: ChatMessageSpec[];
  /** Optional channel prepended as a leading system message when non-empty. */
  systemFrom?: string;
};

const chatMessageBuilderHandler =
  (params: ChatMessageBuilderParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const messages: { role: string; content: string }[] = [];
    if (params.systemFrom !== undefined) {
      const system = params.systemFrom in channels ? valueToText(channels[params.systemFrom]) : "";
      if (system.length > 0) {
        messages.push({ role: "system", content: system });
      }
    }
    for (const spec of params.messages) {
      let content: string;
      if (spec.content !== undefined) {
        content = renderTemplate(spec.content, channels);
      } else if (spec.contentFrom !== undefined) {
        content = spec.contentFrom in channels ? valueToText(channels[spec.contentFrom]) : "";
      } else {
        content = "";
      }
      messages.push({ role: spec.role, content });
    }
    return { [params.into]: messages };
  };

// --- conditionalRouter -------------------------------------------------------

/** One branch for {@link components.conditionalRouter}. */
export type ConditionalRouterBranch = {
  /** The predicate evaluated against the channels (`field` is a dotted path). */
  when: { field: string; op: PredicateOp; value?: unknown };
  /** The route string emitted when `when` holds. */
  route: string;
};

/** Params for {@link components.conditionalRouter}. */
export type ConditionalRouterParams = {
  /** Channel the chosen route string is written into. */
  into: string;
  /** Route emitted when no branch matches. */
  defaultRoute: string;
  /** Ordered branches; the first matching branch wins. */
  branches: ConditionalRouterBranch[];
};

const conditionalRouterHandler =
  (params: ConditionalRouterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const chosen =
      params.branches.find((branch) =>
        predicateHolds(channels, branch.when.field, branch.when.op, branch.when.value)
      )?.route ?? params.defaultRoute;
    return { [params.into]: chosen };
  };

// --- documentWriter ----------------------------------------------------------

/** Params for {@link components.documentWriter}. */
export type DocumentWriterParams = {
  /** Channel holding the incoming documents array to append. */
  from: string;
  /** Channel receiving the accumulated store array. */
  into: string;
  /** Channel holding the current store. Defaults to `into`. */
  store?: string;
  /** Optional object field to de-duplicate the merged store by. */
  dedupeBy?: string;
};

const documentWriterHandler =
  (params: DocumentWriterParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const storeChannel = params.store ?? params.into;
    let docs = [...arrayChannel(channels, storeChannel), ...arrayChannel(channels, params.from)];
    if (params.dedupeBy !== undefined) {
      docs = dedupeArray(docs, params.dedupeBy);
    }
    return { [params.into]: docs };
  };

// --- integration components (httpFetch / webSearch) --------------------------

/**
 * The result of an HTTP fetch surfaced into the `into` channel.
 *
 * The default impl never throws on a non-2xx response or a transport error — a
 * failure is surfaced as data so a graph degrades gracefully instead of crashing
 * the run:
 *  - on a completed response: `{ status, ok, body, json }` (`json` is the parsed
 *    body when the `content-type` is JSON, else `undefined`; `ok` mirrors
 *    `Response.ok`, i.e. a 2xx status);
 *  - on a transport error / timeout: `{ ok: false, error }` (`status`/`body` absent).
 */
export type HttpFetchResult = {
  /** The HTTP status code, when a response was received. */
  status?: number;
  /** `true` for a 2xx response; `false` on a non-2xx response or a transport error. */
  ok: boolean;
  /** The response body as text, when a response was received. */
  body?: string;
  /** The parsed JSON body, present only when the response `content-type` was JSON. */
  json?: unknown;
  /** The error message, present only on a transport error / timeout. */
  error?: string;
};

/**
 * The transport an {@link HttpFetchImpl} is invoked with: the resolved URL plus the
 * request options the default impl assembled from the params (method/headers/body/
 * the abort `signal` driving the timeout). Mirrors the `(input, init)` shape of the
 * WHATWG `fetch`, so `globalThis.fetch` is itself a valid impl.
 */
export type HttpFetchRequestInit = {
  method: string;
  headers?: Record<string, string>;
  body?: string;
  signal?: AbortSignal;
};

/**
 * An injectable fetch implementation. Receives the resolved URL and the assembled
 * request init, and must resolve to something `Response`-shaped (`status`, `ok`,
 * `text()`, `headers.get()`). The real `globalThis.fetch` satisfies this — the
 * default impl simply calls it — so a test can inject a fake `Response`-like object.
 */
export type HttpFetchImpl = (
  url: string,
  init: HttpFetchRequestInit
) => Promise<HttpFetchResponseLike> | HttpFetchResponseLike;

/** The minimal `Response`-shaped surface the default httpFetch impl consumes. */
export type HttpFetchResponseLike = {
  status: number;
  ok: boolean;
  text(): Promise<string> | string;
  headers: { get(name: string): string | null };
};

/** Params for {@link components.httpFetch}. */
export type HttpFetchParams = {
  /** A literal URL to fetch (mutually exclusive with `urlFrom`). */
  url?: string;
  /** A channel whose value supplies the URL (takes precedence when its channel is set). */
  urlFrom?: string;
  /** Channel receiving the {@link HttpFetchResult}. */
  into: string;
  /** HTTP method. Defaults to `"GET"`. */
  method?: string;
  /** Request headers sent with the call. */
  headers?: Record<string, string>;
  /** Request body (sent verbatim) for non-GET methods. */
  body?: string;
  /** Abort the request after this many milliseconds (drives an `AbortController`). */
  timeoutMs?: number;
  /**
   * The transport to call. Defaults to the real `globalThis.fetch`. Inject a fake
   * (a `Response`-like returning function) to keep a test offline, or to point the
   * call at a stub transport.
   */
  fetchImpl?: HttpFetchImpl;
};

/**
 * Whether a `content-type` header denotes JSON (so the body should be parsed):
 * `application/json` or any `+json` suffix (e.g. `application/ld+json`).
 */
const isJsonContentType = (contentType: string | null): boolean => {
  if (contentType === null) {
    return false;
  }
  const lower = contentType.toLowerCase();
  return lower.includes("application/json") || lower.includes("+json");
};

const httpFetchHandler =
  (params: HttpFetchParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const fromChannel =
      params.urlFrom !== undefined && params.urlFrom in channels
        ? valueToText(channels[params.urlFrom])
        : "";
    const url = fromChannel.length > 0 ? fromChannel : (params.url ?? "");
    const impl = params.fetchImpl ?? (globalThis.fetch as HttpFetchImpl);

    // Drive the timeout with an AbortController so a hung request never wedges the run.
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (params.timeoutMs !== undefined && params.timeoutMs > 0) {
      timer = setTimeout(() => controller.abort(), params.timeoutMs);
    }

    const init: HttpFetchRequestInit = {
      method: params.method ?? "GET",
      signal: controller.signal
    };
    if (params.headers !== undefined) {
      init.headers = params.headers;
    }
    if (params.body !== undefined) {
      init.body = params.body;
    }

    try {
      const response = await impl(url, init);
      const text = await response.text();
      const contentType = response.headers.get("content-type");
      let json: unknown;
      if (isJsonContentType(contentType)) {
        try {
          json = JSON.parse(text) as unknown;
        } catch {
          json = undefined;
        }
      }
      const result: HttpFetchResult = { status: response.status, ok: response.ok, body: text };
      if (json !== undefined) {
        result.json = json;
      }
      return { [params.into]: result };
    } catch (error) {
      // Never throw: surface the failure as data so the graph degrades gracefully.
      const message = error instanceof Error ? error.message : String(error);
      const result: HttpFetchResult = { ok: false, error: message };
      return { [params.into]: result };
    } finally {
      if (timer !== undefined) {
        clearTimeout(timer);
      }
    }
  };

/** One web-search result: a title, a url and a snippet. */
export type WebSearchResult = { title: string; url: string; snippet: string };

/**
 * What the web-search connector writes into the `into` channel: the normalized
 * `results` plus an optional `note`. The default connector populates `note` when it
 * degrades gracefully (e.g. no `TAVILY_API_KEY`), leaving `results` empty so a graph
 * runs without crashing.
 */
export type WebSearchOutcome = { results: WebSearchResult[]; note?: string };

/**
 * An injectable web-search implementation. Receives the query + `k`, returns either a
 * bare `WebSearchResult[]` (which is wrapped as `{ results }`) or a full
 * {@link WebSearchOutcome} (so an impl can attach its own `note`).
 */
export type WebSearchImpl = (
  query: string,
  k: number
) => Promise<WebSearchResult[] | WebSearchOutcome> | WebSearchResult[] | WebSearchOutcome;

/**
 * The minimal transport the default Tavily connector posts through: a function with
 * the WHATWG `fetch` `(url, init)` shape resolving to a `Response`-like object. The
 * real `globalThis.fetch` satisfies it; a test injects a fake to stay offline.
 */
export type WebSearchTransport = (
  url: string,
  init: { method: string; headers: Record<string, string>; body: string }
) => Promise<HttpFetchResponseLike> | HttpFetchResponseLike;

/** Params for {@link components.webSearch}. */
export type WebSearchParams = {
  /** A literal query (mutually exclusive with `queryFrom`). */
  query?: string;
  /** A channel whose value supplies the query (takes precedence when its channel is set). */
  queryFrom?: string;
  /** Channel receiving the {@link WebSearchOutcome}. */
  into: string;
  /** Number of results to request. Defaults to 3. */
  k?: number;
  /**
   * The search implementation to call. Defaults to a real Tavily connector behind the
   * `TAVILY_API_KEY` env var: when the key is set it POSTs to the Tavily API; when it
   * is absent it degrades gracefully (no network, empty results + a note). Inject a
   * fake to keep a test offline or to point at another provider.
   */
  searchImpl?: WebSearchImpl;
  /**
   * The HTTP transport the default Tavily connector posts through. Defaults to the real
   * `globalThis.fetch`. Inject a fake to exercise the connector offline. Ignored when
   * `searchImpl` is supplied.
   */
  transport?: WebSearchTransport;
};

/** The Tavily search endpoint the default connector POSTs to. */
const TAVILY_SEARCH_URL = "https://api.tavily.com/search";

/** The graceful-degradation note emitted when no `TAVILY_API_KEY` is configured. */
const NO_SEARCH_KEY_NOTE = "no TAVILY_API_KEY; web search disabled";

/** Coerce one raw Tavily result object into a normalized {@link WebSearchResult}. */
const normalizeTavilyResult = (raw: unknown): WebSearchResult => {
  const obj = typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {};
  const title = typeof obj.title === "string" ? obj.title : "";
  const url = typeof obj.url === "string" ? obj.url : "";
  const snippet = typeof obj.content === "string" ? obj.content : "";
  return { title, url, snippet };
};

/**
 * The default web-search connector: a real Tavily connector behind `TAVILY_API_KEY`.
 *
 * HONEST FALLBACK — when no key is configured it makes **no** network call and returns
 * `{ results: [], note }` so a graph degrades gracefully. When the key is present it
 * POSTs `{ api_key, query, max_results }` to the Tavily API through `transport`
 * (default `globalThis.fetch`) and normalizes `results -> [{ title, url, snippet }]`.
 * On a non-2xx response or a transport error it surfaces `{ results: [], note }`
 * rather than throwing.
 */
const tavilySearch = async (
  query: string,
  k: number,
  transport: WebSearchTransport
): Promise<WebSearchOutcome> => {
  const apiKey = process.env.TAVILY_API_KEY;
  if (apiKey === undefined || apiKey.length === 0) {
    return { results: [], note: NO_SEARCH_KEY_NOTE };
  }
  try {
    const response = await transport(TAVILY_SEARCH_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ api_key: apiKey, query, max_results: k })
    });
    if (!response.ok) {
      return { results: [], note: `tavily search failed with status ${response.status}` };
    }
    const text = await response.text();
    const parsed = JSON.parse(text) as unknown;
    const rawResults =
      typeof parsed === "object" && parsed !== null && Array.isArray((parsed as { results?: unknown }).results)
        ? ((parsed as { results: unknown[] }).results)
        : [];
    return { results: rawResults.slice(0, k).map(normalizeTavilyResult) };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return { results: [], note: `tavily search error: ${message}` };
  }
};

const webSearchHandler =
  (params: WebSearchParams): NodeHandler =>
  async (_input, state) => {
    const channels = channelsOf(state);
    const fromChannel =
      params.queryFrom !== undefined && params.queryFrom in channels
        ? valueToText(channels[params.queryFrom])
        : "";
    const query = fromChannel.length > 0 ? fromChannel : (params.query ?? "");
    const k = params.k ?? 3;

    let outcome: WebSearchResult[] | WebSearchOutcome;
    if (params.searchImpl !== undefined) {
      outcome = await params.searchImpl(query, k);
    } else {
      const transport = params.transport ?? (globalThis.fetch as WebSearchTransport);
      outcome = await tavilySearch(query, k, transport);
    }
    // A bare array is wrapped as `{ results }`; a full outcome passes through.
    const normalized: WebSearchOutcome = Array.isArray(outcome) ? { results: outcome } : outcome;
    return { [params.into]: normalized };
  };

// --- the surface -------------------------------------------------------------

/**
 * The component factory surface. Each factory validates nothing here (the Rust
 * `ComponentRegistry::build_handler` validates `params` at graph-build time, and the
 * TS handlers tolerate the same shapes), returning a {@link ComponentDescriptor}
 * carrying both the native carrier and a faithful TS handler.
 */
export const components = {
  /** Render `{{var}}` placeholders from the channels into a target channel. */
  promptBuilder(params: PromptBuilderParams): ComponentDescriptor {
    return { kind: "promptBuilder", params: { ...params }, handler: promptBuilderHandler(params) };
  },
  /** Validate a channel value's type / required keys, writing an ok flag + errors. */
  jsonValidator(params: JsonValidatorParams): ComponentDescriptor {
    return { kind: "jsonValidator", params: { ...params }, handler: jsonValidatorHandler(params) };
  },
  /** Extract the first JSON object/array from a text channel into a target channel. */
  outputParser(params: OutputParserParams): ComponentDescriptor {
    return { kind: "outputParser", params: { ...params }, handler: outputParserHandler(params) };
  },
  /** Pick a route string from a channel value (pairs with a conditional edge). */
  router(params: RouterParams): ComponentDescriptor {
    return { kind: "router", params: { ...params }, handler: routerHandler(params) };
  },
  /** Score candidate docs against a query and keep the top-`k`. */
  retriever(params: RetrieverParams): ComponentDescriptor {
    return { kind: "retriever", params: { ...params }, handler: retrieverHandler(params) };
  },
  /** Reorder a retrieval-result array, optionally re-scoring against a query. */
  reranker(params: RerankerParams): ComponentDescriptor {
    return { kind: "reranker", params: { ...params }, handler: rerankerHandler(params) };
  },
  /** Normalise a text channel: strip HTML, lowercase, collapse whitespace, trim. */
  textCleaner(params: TextCleanerParams): ComponentDescriptor {
    return { kind: "textCleaner", params: { ...params }, handler: textCleanerHandler(params) };
  },
  /** Split a text channel into an array of chunk strings by chars or sentences. */
  documentSplitter(params: DocumentSplitterParams): ComponentDescriptor {
    return {
      kind: "documentSplitter",
      params: { ...params },
      handler: documentSplitterHandler(params)
    };
  },
  /** Strip HTML tags from a text channel and decode the common named entities. */
  htmlToText(params: HtmlToTextParams): ComponentDescriptor {
    return { kind: "htmlToText", params: { ...params }, handler: htmlToTextHandler(params) };
  },
  /** Parse a CSV text channel into an array of row objects (or arrays). */
  csvParser(params: CsvParserParams): ComponentDescriptor {
    return { kind: "csvParser", params: { ...params }, handler: csvParserHandler(params) };
  },
  /** Concatenate the array values across several channels into one merged array. */
  documentJoiner(params: DocumentJoinerParams): ComponentDescriptor {
    return { kind: "documentJoiner", params: { ...params }, handler: documentJoinerHandler(params) };
  },
  /** De-duplicate an array channel, keeping the first occurrence and order. */
  deduplicator(params: DeduplicatorParams): ComponentDescriptor {
    return { kind: "deduplicator", params: { ...params }, handler: deduplicatorHandler(params) };
  },
  /** Truncate a text channel to at most `maxChars` characters with an ellipsis. */
  truncator(params: TruncatorParams): ComponentDescriptor {
    return { kind: "truncator", params: { ...params }, handler: truncatorHandler(params) };
  },
  /** Extract literal-pattern matches (with `^`/`$` anchors) from a text channel. */
  regexExtractor(params: RegexExtractorParams): ComponentDescriptor {
    return { kind: "regexExtractor", params: { ...params }, handler: regexExtractorHandler(params) };
  },
  /** Assemble a final answer string, optionally appending numbered citations. */
  answerBuilder(params: AnswerBuilderParams): ComponentDescriptor {
    return { kind: "answerBuilder", params: { ...params }, handler: answerBuilderHandler(params) };
  },
  /** Remap an object channel's fields (by dotted path) into a new object. */
  fieldMapper(params: FieldMapperParams): ComponentDescriptor {
    return { kind: "fieldMapper", params: { ...params }, handler: fieldMapperHandler(params) };
  },
  /** Extract a scalar from a channel (optional dotted path; finalOnly reduces an agent trace to its final answer). */
  fieldExtractor(params: FieldExtractorParams): ComponentDescriptor {
    return { kind: "fieldExtractor", params: { ...params }, handler: fieldExtractorHandler(params) };
  },
  /** Lexical BM25 ranking of a corpus against a query; keep the top-`k`. */
  bm25Retriever(params: Bm25RetrieverParams): ComponentDescriptor {
    return { kind: "bm25Retriever", params: { ...params }, handler: bm25RetrieverHandler(params) };
  },
  /** Keyword-overlap ranking of a corpus against a query; keep the top-`k`. */
  keywordRetriever(params: KeywordRetrieverParams): ComponentDescriptor {
    return {
      kind: "keywordRetriever",
      params: { ...params },
      handler: keywordRetrieverHandler(params)
    };
  },
  /** Split text into overlapping windows of whole sentences. */
  sentenceWindowSplitter(params: SentenceWindowSplitterParams): ComponentDescriptor {
    return {
      kind: "sentenceWindowSplitter",
      params: { ...params },
      handler: sentenceWindowSplitterHandler(params)
    };
  },
  /** Heuristic language detection over a small set of common languages. */
  languageDetector(params: LanguageDetectorParams): ComponentDescriptor {
    return {
      kind: "languageDetector",
      params: { ...params },
      handler: languageDetectorHandler(params)
    };
  },
  /** Filter an array channel by a dotted-path metadata predicate. */
  metadataFilter(params: MetadataFilterParams): ComponentDescriptor {
    return { kind: "metadataFilter", params: { ...params }, handler: metadataFilterHandler(params) };
  },
  /** Combine several array channels by concat / union / interleave. */
  listJoiner(params: ListJoinerParams): ComponentDescriptor {
    return { kind: "listJoiner", params: { ...params }, handler: listJoinerHandler(params) };
  },
  /** Fuse several retrieval streams into one ranking with Reciprocal Rank Fusion. */
  mergeRanker(params: MergeRankerParams): ComponentDescriptor {
    return { kind: "mergeRanker", params: { ...params }, handler: mergeRankerHandler(params) };
  },
  /** Score actual vs expected text (token-F1 / overlap / exact), with an optional pass flag. */
  evaluator(params: EvaluatorParams): ComponentDescriptor {
    return { kind: "evaluator", params: { ...params }, handler: evaluatorHandler(params) };
  },
  /** Assemble a role-tagged chat-message array an LLM generator consumes. */
  chatMessageBuilder(params: ChatMessageBuilderParams): ComponentDescriptor {
    return {
      kind: "chatMessageBuilder",
      params: { ...params },
      handler: chatMessageBuilderHandler(params)
    };
  },
  /** Multi-branch rule routing over the channels (pairs with a conditional edge). */
  conditionalRouter(params: ConditionalRouterParams): ComponentDescriptor {
    return {
      kind: "conditionalRouter",
      params: { ...params },
      handler: conditionalRouterHandler(params)
    };
  },
  /** Append documents into an in-state document store array (optionally de-duplicating). */
  documentWriter(params: DocumentWriterParams): ComponentDescriptor {
    return { kind: "documentWriter", params: { ...params }, handler: documentWriterHandler(params) };
  },
  /**
   * **Integration component (vendor I/O).** Perform an HTTP request, writing an
   * {@link HttpFetchResult} (`{ status, ok, body, json }`) to `into`. This is **not**
   * a Rust component: it returns a plain {@link NodeHandler} added with
   * {@link import("./builder.js").GraphBuilder.node} and runs over the JS seam on the
   * Rust engine. The default transport is the real `globalThis.fetch` (supports
   * `method`/`headers`/`body`/`timeoutMs` via an `AbortController`); it never throws —
   * a non-2xx is surfaced via `ok`/`status`, and an error/timeout writes
   * `{ ok: false, error }`. Inject `fetchImpl` to override the transport (e.g. offline
   * in tests).
   */
  httpFetch(params: HttpFetchParams): IntegrationComponentHandler {
    return httpFetchHandler(params);
  },
  /**
   * **Integration component (vendor I/O).** Run a web search, writing a
   * {@link WebSearchOutcome} (`{ results, note? }`) to `into`. This is **not** a Rust
   * component: it returns a plain {@link NodeHandler} added with
   * {@link import("./builder.js").GraphBuilder.node} and runs over the JS seam on the
   * Rust engine. The default is a real Tavily connector behind `TAVILY_API_KEY`
   * (POSTs to `https://api.tavily.com/search`, normalizing results to
   * `[{ title, url, snippet }]`); when the key is absent it degrades gracefully with
   * **no** network call (empty results + a note). Inject `searchImpl` to override the
   * provider, or `transport` to keep the Tavily connector offline.
   */
  webSearch(params: WebSearchParams): IntegrationComponentHandler {
    return webSearchHandler(params);
  }
} as const;
