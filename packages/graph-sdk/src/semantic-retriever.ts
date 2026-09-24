/**
 * A semantic (vector-store) retrieval connector as an exported SDK helper — the "real
 * embeddings" sibling of the deterministic mock `components.retriever`. It is NOT a new
 * catalog component kind: {@link semanticRetriever} returns a plain {@link NodeHandler}
 * added with {@link import("./builder.js").GraphBuilder.node} (the same vendor-I/O shape
 * as `components.httpFetch` / `components.webSearch`), so on the Rust engine it runs over
 * the async JS seam (`on_node`) like any other JS node.
 *
 * On each run it (optionally) embeds `docs` into the {@link VectorStore}, embeds the
 * query, then writes the top-`k` `{ id, content, score }` matches to the `into` channel.
 * `embeddings` defaults to a real Mistral client ({@link createEmbeddings}); inject a
 * fake {@link Embeddings} to keep a test offline and deterministic.
 *
 * ```ts
 * import { createGraph, semanticRetriever } from "@ailu-ai/graph-sdk";
 *
 * createGraph({ name: "semantic" })
 *   .channel("q", { type: "string", default: "" })
 *   .channel("hits", { type: "json", default: [] })
 *   .node("retrieve", semanticRetriever({
 *     queryFrom: "q",
 *     into: "hits",
 *     k: 3,
 *     docs: [{ id: "d1", content: "Ailu is a graph runtime." }],
 *     embeddings: fakeEmbeddings // deterministic in tests
 *   }));
 * ```
 */

import type { NodeHandler } from "@ailu-ai/graph-runtime";

import { createEmbeddings, type Embeddings } from "./embeddings.js";
import { createVectorStore, type VectorStore, type VectorStoreMatch } from "./vector-store.js";

/** A candidate document to embed into the store before querying. */
export type SemanticRetrieverDoc = { id: string; content: string };

/** Params for {@link semanticRetriever}. */
export type SemanticRetrieverParams = {
  /** A literal query (mutually exclusive with `queryFrom`). */
  query?: string;
  /** A channel whose value supplies the query (takes precedence when its channel is set). */
  queryFrom?: string;
  /** Channel receiving the top-`k` `{ id, content, score }` array. */
  into: string;
  /** Number of results to keep. Defaults to 4. */
  k?: number;
  /**
   * Documents to embed into the store before querying. Each run embeds and upserts these
   * (idempotent by id). Omit to query against an already-populated injected `store`.
   */
  docs?: SemanticRetrieverDoc[];
  /**
   * The vector store to upsert into / query. Defaults to a fresh in-memory store created
   * per factory call. Inject a shared/persistent store to reuse embeddings across runs.
   */
  store?: VectorStore;
  /**
   * The embeddings client. Defaults to a real Mistral client ({@link createEmbeddings}).
   * Inject a fake (deterministic vectors) to keep a test offline.
   */
  embeddings?: Embeddings;
};

/**
 * Coerce a channel value to text the way the rest of the SDK does: strings pass through,
 * `null`/`undefined` become the empty string, everything else is compact JSON.
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

/**
 * Build the semantic-retriever node handler. Each invocation embeds any supplied `docs`
 * into the store, embeds the resolved query, and writes the top-`k`
 * {@link VectorStoreMatch} array (`{ id, content, score }`) to `into`. Throws no special
 * error class — it surfaces the underlying embeddings error if the real client can't run
 * (no key / no transport), which is the honest failure mode for a real connector.
 */
export const semanticRetriever = (params: SemanticRetrieverParams): NodeHandler => {
  // Default the store/embeddings once at factory time so repeated runs of the same node
  // reuse the same in-memory store (embeddings are upserted idempotently by id).
  const store = params.store ?? createVectorStore();
  const embeddings = params.embeddings ?? createEmbeddings();

  return async (_input, state) => {
    const channels = channelsOf(state);
    const fromChannel =
      params.queryFrom !== undefined && params.queryFrom in channels
        ? valueToText(channels[params.queryFrom])
        : "";
    const query = fromChannel.length > 0 ? fromChannel : (params.query ?? "");
    const k = params.k ?? 4;

    const docs = params.docs ?? [];
    if (docs.length > 0) {
      const vectors = await embeddings.embed(docs.map((doc) => doc.content));
      store.upsert(
        docs.map((doc, index) => ({
          id: doc.id,
          content: doc.content,
          embedding: vectors[index] ?? []
        }))
      );
    }

    const [queryVector] = await embeddings.embed([query]);
    const matches: VectorStoreMatch[] =
      queryVector === undefined ? [] : store.query(queryVector, k);
    // Project to the public `{ id, content, score }` shape (drop metadata for the channel).
    const results = matches.map((match) => ({ id: match.id, content: match.content, score: match.score }));
    return { [params.into]: results };
  };
};
