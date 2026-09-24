/**
 * Real text embeddings as an exported SDK helper (NOT a catalog component kind).
 *
 * {@link createEmbeddings} returns an {@link Embeddings} whose `embed` turns a batch of
 * texts into dense vectors. The default transport POSTs to the provider's embeddings API
 * (`/embeddings` with `{ model, input }` and a `Bearer` key), parsing `data[].embedding` —
 * both Mistral and OpenAI's embeddings APIs share this exact response shape, so ONE parser
 * and ONE transport builder serve both. The {@link CreateEmbeddingsOptions.transport} hook
 * overrides the network call so a test can return deterministic vectors with no real
 * network. This is the embedding backbone behind
 * {@link import("./semantic-retriever.js").semanticRetriever}.
 *
 * ```ts
 * import { createEmbeddings } from "@ailu-ai/graph-sdk";
 *
 * const embeddings = createEmbeddings({ apiKey: process.env.MISTRAL_API_KEY });
 * const [a, b] = await embeddings.embed(["hello", "world"]);
 *
 * // A second provider (issue #541 — product-side gap: only one provider was ever wired):
 * const openai = createEmbeddings({ provider: "openai", apiKey: process.env.OPENAI_API_KEY });
 * ```
 */

/** An embedder: turn a batch of texts into one dense vector each (order-preserving). */
export type Embeddings = {
  /** Embed `texts` into a `number[][]` of the same length and order. */
  embed(texts: string[]): Promise<number[][]>;
};

/**
 * The transport an embeddings client posts through: it receives the assembled request
 * body and must resolve to the parsed JSON response (the `{ data: [{ embedding }] }`
 * shape both Mistral's and OpenAI's embeddings APIs return). The real default builds this
 * from `fetch`; a test injects a fake to stay offline.
 */
export type EmbeddingsTransport = (body: EmbeddingsRequestBody) => Promise<unknown> | unknown;

/** The request body POSTed to the embeddings endpoint (`{ model, input }`). `dimensions` is
 *  OpenAI-specific (its `text-embedding-3-*` models support down-projecting via this field);
 *  omitted from the body entirely when not set, so it's a no-op for a provider that ignores it. */
export type EmbeddingsRequestBody = {
  model: string;
  input: string[];
  dimensions?: number;
};

/** The embeddings providers `createEmbeddings` knows how to reach directly. */
export type EmbeddingsProvider = "mistral" | "openai";

type ProviderDefaults = { model: string; baseUrl: string; envVar: string };

const PROVIDER_DEFAULTS: Record<EmbeddingsProvider, ProviderDefaults> = {
  mistral: { model: "mistral-embed", baseUrl: "https://api.mistral.ai/v1", envVar: "MISTRAL_API_KEY" },
  openai: {
    model: "text-embedding-3-small",
    baseUrl: "https://api.openai.com/v1",
    envVar: "OPENAI_API_KEY"
  }
};

/** Options for {@link createEmbeddings}. */
export type CreateEmbeddingsOptions = {
  /** The embeddings provider. Defaults to `"mistral"` (unchanged from before this option existed). */
  provider?: EmbeddingsProvider;
  /** API key. Defaults to `process.env.MISTRAL_API_KEY`/`OPENAI_API_KEY` per `provider`. Required
   *  unless `transport` is injected. */
  apiKey?: string;
  /** Embedding model. Defaults to the provider's own default (`mistral-embed` / `text-embedding-3-small`). */
  model?: string;
  /** API base URL. Defaults to the provider's own default. */
  baseUrl?: string;
  /** Down-project the output vectors to this many dimensions (OpenAI `text-embedding-3-*` only —
   *  a provider that doesn't support it silently ignores an unset field, never sent unless set). */
  dimensions?: number;
  /**
   * An injectable transport overriding the default `fetch`-based call. Receives the
   * request body and returns the parsed JSON response. Inject a fake to keep a test
   * offline (or to point at a stub) — when set, no API key is required.
   */
  transport?: EmbeddingsTransport;
};

/** Raised when no API key and no transport were supplied, so a real call is impossible. */
export class MissingEmbeddingsKeyError extends Error {
  public constructor(provider: EmbeddingsProvider = "mistral", envVar: string = "MISTRAL_API_KEY") {
    super(
      `createEmbeddings: no API key (set \`apiKey\` or ${envVar}) and no \`transport\` injected for ` +
        `provider "${provider}"; supply one to make a real call, or inject \`transport\` for offline use.`
    );
    this.name = "MissingEmbeddingsKeyError";
  }
}

/** Raised when the embeddings response doesn't carry the expected `data[].embedding` shape. */
export class EmbeddingsResponseError extends Error {
  public constructor(detail: string) {
    super(`createEmbeddings: malformed embeddings response: ${detail}`);
    this.name = "EmbeddingsResponseError";
  }
}

/**
 * Parse a `{ data: [{ embedding: number[] }, …] }` response into a `number[][]`,
 * preserving order. Throws an {@link EmbeddingsResponseError} when the shape is wrong.
 */
const parseEmbeddingsResponse = (parsed: unknown): number[][] => {
  if (typeof parsed !== "object" || parsed === null) {
    throw new EmbeddingsResponseError("response is not an object");
  }
  const data = (parsed as { data?: unknown }).data;
  if (!Array.isArray(data)) {
    throw new EmbeddingsResponseError("`data` is not an array");
  }
  return data.map((entry, index) => {
    const embedding =
      typeof entry === "object" && entry !== null
        ? (entry as { embedding?: unknown }).embedding
        : undefined;
    if (!Array.isArray(embedding) || !embedding.every((n) => typeof n === "number")) {
      throw new EmbeddingsResponseError(`\`data[${index}].embedding\` is not a number[]`);
    }
    return embedding as number[];
  });
};

/**
 * The default `fetch`-based transport: POST `{ model, input, dimensions? }` to
 * `{baseUrl}/embeddings` with an `Authorization: Bearer <key>` header, returning the
 * parsed JSON. Identical wire shape for Mistral and OpenAI (both accept/return the same
 * `{model, input}` request / `{data: [{embedding}]}` response — `dimensions` is simply
 * unknown-and-ignored by a provider that doesn't support it). Throws on a non-2xx status.
 * Only built when no `transport` is injected.
 */
const createBearerJsonTransport =
  (apiKey: string, baseUrl: string): EmbeddingsTransport =>
  async (body) => {
    const response = await globalThis.fetch(`${baseUrl}/embeddings`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${apiKey}`
      },
      body: JSON.stringify(body)
    });
    const text = await response.text();
    if (!response.ok) {
      throw new EmbeddingsResponseError(`status ${response.status}: ${text}`);
    }
    return JSON.parse(text) as unknown;
  };

/**
 * Create an {@link Embeddings} client for `options.provider` (defaults to `"mistral"`,
 * unchanged from before this option existed). With the default transport it POSTs to the
 * provider's own base URL with `{ model, input: texts, dimensions? }` and `Authorization:
 * Bearer (apiKey || process.env[<provider's env var>])`, parsing `data[].embedding` (the
 * same response shape for both wired providers). Inject `transport` to override that for
 * offline tests. Throws {@link MissingEmbeddingsKeyError} when neither a key nor a
 * transport is available.
 */
export const createEmbeddings = (options: CreateEmbeddingsOptions = {}): Embeddings => {
  const provider = options.provider ?? "mistral";
  const defaults = PROVIDER_DEFAULTS[provider];
  const model = options.model ?? defaults.model;
  const baseUrl = options.baseUrl ?? defaults.baseUrl;
  const dimensions = options.dimensions;

  let transport: EmbeddingsTransport;
  if (options.transport !== undefined) {
    transport = options.transport;
  } else {
    const apiKey = options.apiKey ?? process.env[defaults.envVar];
    if (apiKey === undefined || apiKey.length === 0) {
      throw new MissingEmbeddingsKeyError(provider, defaults.envVar);
    }
    transport = createBearerJsonTransport(apiKey, baseUrl);
  }

  return {
    async embed(texts) {
      if (texts.length === 0) {
        return [];
      }
      const parsed = await transport({
        model,
        input: texts,
        ...(dimensions !== undefined ? { dimensions } : {})
      });
      return parseEmbeddingsResponse(parsed);
    }
  };
};
