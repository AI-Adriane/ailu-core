/**
 * `@ailu-ai/model-core` — the shared base for Ailu's per-provider model packages
 * (`@ailu-ai/model-openai`, `-anthropic`, `-gemini`, `-mistral`), ADR 0031.
 *
 * Ailu runs on **one Rust engine**. A model package is a thin SDK **overlay**: it declares
 * a serializable {@link ModelSpec} (provider + model + tier) you pass to `agentNode({ model })`
 * — the Rust engine executes the graph — and it can be called standalone via {@link Model.invoke}
 * / {@link Model.stream}, which route a one-shot through the Rust gateway over the napi seam.
 * The HTTP happens in Rust: no TS provider client, no second engine, one consistent behaviour.
 */

import { createRequire } from "node:module";

/** Provider slugs the compiled-in Rust adapters understand (mirrors the Rust `LlmProvider`). */
export type ProviderSlug =
  | "openai"
  | "anthropic"
  | "google"
  | "mistral"
  | "openrouter"
  | "minimax"
  | "huggingface"
  | "ollama"
  | "lmstudio";

const KNOWN_PROVIDERS: ReadonlySet<string> = new Set<ProviderSlug>([
  "openai",
  "anthropic",
  "google",
  "mistral",
  "openrouter",
  "minimax",
  "huggingface",
  "ollama",
  "lmstudio"
]);

/** Capability tier (mirrors the Rust `ModelPolicy` tiers). */
export type ModelTier = "fast" | "balanced" | "frontier" | "creative";

/**
 * A serializable model declaration — the authoring surface that round-trips through both the
 * napi (TS) and pyo3 (Python) seams. `baseURL` points an OpenAI-wire provider at a custom endpoint
 * (vLLM, LM Studio, a gateway, …) on both the graph path and `invoke()`; the key for that endpoint
 * is read only from `apiKeyEnv` (unset → keyless), never from the provider's default key variable.
 */
export type ModelSpec = {
  /** Provider slug. **Optional** (ADR 0034): omit it with a `tier` (or nothing) to let the engine
   * pick the provider from whichever API keys are present in the environment — `model.fast` /
   * `model.invoke()`. An explicit unknown provider fails loud (never silent-Anthropic). */
  provider?: ProviderSlug;
  model?: string;
  tier?: ModelTier;
  baseURL?: string;
  apiKeyEnv?: string;
};

/** An unknown provider slug reached the SDK (ADR 0034 — fail loud, never silent-Anthropic).
 * Carries a stable `code` + actionable `hint` (errors-that-teach). */
export class UnknownProviderError extends Error {
  readonly code = "AILU_UNKNOWN_PROVIDER";
  readonly hint: string;
  constructor(
    readonly provider: string,
    readonly knownProviders: readonly string[]
  ) {
    super(
      `Unknown provider "${provider}". Known: ${knownProviders.join(", ")}. ` +
        `For a custom OpenAI-compatible endpoint use model.openaiCompatible({ baseURL }).`
    );
    this.name = "UnknownProviderError";
    this.hint = `Use one of: ${knownProviders.join(", ")} — or model.openaiCompatible({ baseURL }) for a custom endpoint.`;
  }
}

/** A named provider has no API key in the environment (ADR 0034). Names the exact env var. */
export class MissingProviderKeyError extends Error {
  readonly code = "AILU_MISSING_PROVIDER_KEY";
  readonly hint: string;
  constructor(
    readonly provider: ProviderSlug,
    readonly envVar: string
  ) {
    super(`No API key for provider "${provider}": set ${envVar} in the environment.`);
    this.name = "MissingProviderKeyError";
    this.hint = `Set ${envVar} in the environment (or pass apiKeyEnv to name a different variable).`;
  }
}

/** A provider-less (tier-only / zero-config) model found no provider key in the env (ADR 0034). */
export class NoProviderInEnvError extends Error {
  readonly code = "AILU_NO_PROVIDER_IN_ENV";
  readonly hint: string;
  constructor(readonly checkedVars: readonly string[]) {
    super(
      `No provider API key found in the environment. Set one of: ${checkedVars.join(", ")} — ` +
        `or name a provider explicitly, e.g. model.openai("gpt-4o").`
    );
    this.name = "NoProviderInEnvError";
    this.hint = `Set one of: ${checkedVars.join(", ")} — or name a provider explicitly, e.g. model.openai("gpt-4o").`;
  }
}

/** Fails loudly on an unknown provider slug (defuses the Rust catch-all silent-Anthropic). */
export function assertKnownProvider(provider: string): asserts provider is ProviderSlug {
  if (!KNOWN_PROVIDERS.has(provider)) {
    throw new UnknownProviderError(provider, [...KNOWN_PROVIDERS]);
  }
}

/**
 * The env var each provider's API key is read from by default (ADR 0034). `null` = keyless
 * (local servers). Overridable per-spec via {@link ModelSpec.apiKeyEnv}. The engine reads the same
 * variables, plus the aliases in {@link KEY_ENV_ALIASES}.
 */
export const DEFAULT_KEY_ENV: Record<ProviderSlug, string | null> = {
  openai: "OPENAI_API_KEY",
  anthropic: "ANTHROPIC_API_KEY",
  google: "GEMINI_API_KEY",
  mistral: "MISTRAL_API_KEY",
  openrouter: "OPENROUTER_API_KEY",
  minimax: "MINIMAX_API_KEY",
  huggingface: "HF_TOKEN",
  ollama: null,
  lmstudio: null
};

/** Other variables a provider's key is also read from, after its {@link DEFAULT_KEY_ENV}. */
export const KEY_ENV_ALIASES: Partial<Record<ProviderSlug, readonly string[]>> = {
  google: ["GOOGLE_API_KEY"],
  huggingface: ["HUGGINGFACE_API_KEY"]
};

/** `AILU_LLM_MOCK=1`: calls with no credentials answer from the engine's deterministic offline
 * mock instead of failing. For tests, CI and trying examples without an API key. */
export const OFFLINE_MOCK_ENV = "AILU_LLM_MOCK";

const offlineMock = (env: Record<string, string | undefined>): boolean => env[OFFLINE_MOCK_ENV] === "1";

/** The first non-empty value among `names` in `env`. */
const firstSet = (
  env: Record<string, string | undefined>,
  names: readonly string[]
): { name: string; value: string } | undefined => {
  for (const name of names) {
    const value = env[name];
    if (value !== undefined && value !== "") return { name, value };
  }
  return undefined;
};

/** Provider preference order when resolving a provider-less spec from the environment. */
const PROVIDER_PREFERENCE: readonly ProviderSlug[] = [
  "anthropic",
  "openai",
  "google",
  "mistral",
  "openrouter",
  "minimax",
  "huggingface"
];

/** A spec resolved to a concrete provider + the provider-keys map to send over the wire. */
export type ResolvedKeys = { provider: ProviderSlug; providerKeys: Record<string, string> };

/**
 * Resolve a spec's provider + API key from the environment (ADR 0034), fail-loud:
 * - explicit provider, keyless (ollama/lmstudio) → no key needed;
 * - explicit provider with a key env present → `{ [provider]: value }`;
 * - explicit provider, key absent → {@link MissingProviderKeyError};
 * - provider-less (tier-only / zero-config) → pick the highest-preference provider whose key is
 *   present; none present → {@link NoProviderInEnvError}. Never defaults to a provider silently.
 * - offline mode (`AILU_LLM_MOCK=1`) turns a missing key into a keyless call the engine answers
 *   from its deterministic mock, instead of an error.
 *
 * `env` defaults to `process.env` (injected for tests). Only ever reads an env var, never a literal.
 */
export function resolveProviderKeys(
  spec: ModelSpec,
  env: Record<string, string | undefined> = process.env
): ResolvedKeys {
  if (spec.baseURL !== undefined && spec.baseURL.trim() !== "") {
    // A custom endpoint gets only the key named by `apiKeyEnv`: `OPENAI_API_KEY` (and friends)
    // are credentials for the provider's public API and must never be sent to another host.
    const provider = spec.provider ?? "openai";
    assertKnownProvider(provider);
    if (spec.apiKeyEnv === undefined || spec.apiKeyEnv === "") {
      return { provider, providerKeys: {} };
    }
    const value = env[spec.apiKeyEnv];
    if (value === undefined || value === "") {
      throw new MissingProviderKeyError(provider, spec.apiKeyEnv);
    }
    return { provider, providerKeys: { [provider]: value } };
  }
  if (spec.provider !== undefined) {
    assertKnownProvider(spec.provider);
    const envVar = spec.apiKeyEnv ?? DEFAULT_KEY_ENV[spec.provider];
    if (envVar === null) {
      return { provider: spec.provider, providerKeys: {} };
    }
    const names =
      spec.apiKeyEnv === undefined ? [envVar, ...(KEY_ENV_ALIASES[spec.provider] ?? [])] : [envVar];
    const found = firstSet(env, names);
    if (found === undefined) {
      // Offline mode: no key is sent, and the engine answers from its deterministic mock.
      if (offlineMock(env)) return { provider: spec.provider, providerKeys: {} };
      throw new MissingProviderKeyError(spec.provider, envVar);
    }
    return { provider: spec.provider, providerKeys: { [spec.provider]: found.value } };
  }
  // Provider-less: pick the highest-preference provider whose key is present.
  const checked: string[] = [];
  for (const provider of PROVIDER_PREFERENCE) {
    const envVar = DEFAULT_KEY_ENV[provider];
    if (envVar === null) continue;
    checked.push(envVar);
    const found = firstSet(env, [envVar, ...(KEY_ENV_ALIASES[provider] ?? [])]);
    if (found !== undefined) {
      return { provider, providerKeys: { [provider]: found.value } };
    }
  }
  if (offlineMock(env)) return { provider: "anthropic", providerKeys: {} };
  throw new NoProviderInEnvError(checked);
}

/** A chat turn for {@link Model.invoke}. */
export type ChatMessage = { role: "system" | "user" | "assistant"; content: string };

/** Options for a one-shot completion. */
export type InvokeOptions = { maxTokens?: number; temperature?: number };

/** Token usage on a {@link ModelResponse}. */
export type ModelUsage = {
  promptTokens: number;
  completionTokens: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
};

/** The result of a one-shot completion (the serialized Rust `LlmResponse`). */
export type ModelResponse = {
  content: string;
  toolCalls?: Array<{ id: string; name: string; input: unknown }>;
  stopReason?: string;
  usage: ModelUsage;
  model: string;
  provider: ProviderSlug;
};

type NapiLlm = { llmComplete(requestJson: string, providerKeysJson: string): Promise<string> };

let cachedNapi: NapiLlm | null | undefined;

function loadNapi(): NapiLlm {
  if (cachedNapi === undefined) {
    try {
      const requireFn = createRequire(import.meta.url);
      const mod = requireFn("@ailu-ai/napi") as Record<string, unknown>;
      cachedNapi = typeof mod.llmComplete === "function" ? (mod as unknown as NapiLlm) : null;
    } catch {
      cachedNapi = null;
    }
  }
  if (!cachedNapi) {
    throw new Error(
      "@ailu-ai/napi (the Rust engine) is not available — Model.invoke()/stream() need it. " +
        "Build it: bash scripts/build-napi.sh"
    );
  }
  return cachedNapi;
}

/** One-shot completion for a {@link ModelSpec} via the Rust gateway (the napi seam).
 *
 * Resolves the provider + API key from the environment (ADR 0034): a provider-less spec picks
 * the highest-preference provider whose key is present; a named provider with no key fails loud.
 * A `responseFormat` (JSON Schema) is forwarded so the engine drives provider-native structured
 * output (ADR 0029). `env` is injected for tests. */
export async function invokeModel(
  spec: ModelSpec,
  input: string | ChatMessage[],
  opts?: InvokeOptions & { responseFormat?: { name?: string; schema: Record<string, unknown> } },
  env: Record<string, string | undefined> = process.env
): Promise<ModelResponse> {
  const { provider, providerKeys } = resolveProviderKeys(spec, env);
  const messages = typeof input === "string" ? [{ role: "user", content: input }] : input;
  const request = {
    provider,
    model: spec.model ?? "",
    // Read by the napi seam next to the `LlmRequest` fields: routes the call to this endpoint.
    baseUrl: spec.baseURL,
    messages,
    maxTokens: opts?.maxTokens,
    temperature: opts?.temperature,
    // Wire shape mirrors the Rust `ResponseFormat::JsonSchema` (serde tag "type", camelCase).
    responseFormat: opts?.responseFormat
      ? {
          type: "jsonSchema",
          name: opts.responseFormat.name ?? "output",
          schema: opts.responseFormat.schema,
          strict: true
        }
      : undefined
  };
  const napi = loadNapi();
  const responseJson = await napi.llmComplete(JSON.stringify(request), JSON.stringify(providerKeys));
  return JSON.parse(responseJson) as ModelResponse;
}

/**
 * The base every per-provider model overlay extends. Carries a {@link ModelSpec}, serializes to
 * it (so `agentNode({ model })` and the napi/pyo3 wire get plain data), and is callable standalone
 * via {@link Model.invoke}. Subclasses set `provider` + sensible defaults + tier helpers.
 */
export abstract class Model {
  /** The serializable declaration this overlay represents. */
  abstract readonly spec: ModelSpec;

  /** The plain {@link ModelSpec} — what the SDK serializes onto the Rust wire. */
  toSpec(): ModelSpec {
    return this.spec;
  }

  /** `JSON.stringify(model)` yields the spec (so a model drops straight into config). */
  toJSON(): ModelSpec {
    return this.spec;
  }

  /** One-shot completion through the Rust gateway. `model.openai("gpt-4o").invoke("hi")`.
   * The answer text is on `.content`; use {@link Model.output} for typed structured output. */
  invoke(input: string | ChatMessage[], opts?: InvokeOptions): Promise<ModelResponse> {
    return invokeModel(this.spec, input, opts);
  }

  /** Constrain output to a JSON Schema (ADR 0034). Returns a model whose `.invoke()` adds a
   * typed `.parsed` — the engine drives provider-native structured output (ADR 0029), the SDK
   * only infers the type. Still a single napi call. `schema` is any `{ jsonSchema, parse }`
   * (a Zod schema wraps to one via the `jsonSchema()` helper in graph-sdk). */
  output<T>(schema: OutputSchema<T>): TypedModel<T> {
    return new TypedModel<T>(this.spec, schema);
  }
}

/** A JSON-Schema + parser for {@link Model.output}. Generic by design — no hard Zod dependency. */
export type OutputSchema<T> = {
  name?: string;
  /** The JSON Schema the engine constrains the response to. */
  jsonSchema: Record<string, unknown>;
  /** Validate/parse the returned JSON into the typed value. */
  parse(value: unknown): T;
};

/** A {@link Model} with a structured-output schema — `.invoke()` returns a typed `.parsed`. */
export class TypedModel<T> extends Model {
  readonly spec: ModelSpec;
  readonly #schema: OutputSchema<T>;
  constructor(spec: ModelSpec, schema: OutputSchema<T>) {
    super();
    this.spec = spec;
    this.#schema = schema;
  }

  override async invoke(
    input: string | ChatMessage[],
    opts?: InvokeOptions
  ): Promise<ModelResponse & { parsed: T }> {
    const res = await invokeModel(this.spec, input, {
      ...opts,
      responseFormat: { name: this.#schema.name, schema: this.#schema.jsonSchema }
    });
    return { ...res, parsed: this.#schema.parse(JSON.parse(res.content)) };
  }
}

/** A concrete {@link Model} wrapping a frozen {@link ModelSpec} — what the `model` namespace returns. */
export class SpecModel extends Model {
  readonly spec: ModelSpec;
  constructor(spec: ModelSpec) {
    super();
    this.spec = Object.freeze({ ...spec });
  }
}

/** A {@link ModelSpec} | {@link Model} | a `"provider:model"` / `"provider:tier"` string — what
 * `agentNode({ model })` and `model()` accept (ADR 0034). */
export type ModelLike = ModelSpec | Model | string;

const TIER_KEYWORDS: ReadonlySet<string> = new Set<ModelTier>([
  "fast",
  "balanced",
  "frontier",
  "creative"
]);

/** Parse a `"provider:rest"` string into a {@link ModelSpec}. `rest` is a tier keyword
 * (`openai:fast`) or a model id (`openai:gpt-4o`). Fails loud on an unknown provider. */
export function parseModelString(value: string): ModelSpec {
  const idx = value.indexOf(":");
  const provider = idx === -1 ? value : value.slice(0, idx);
  const rest = idx === -1 ? "" : value.slice(idx + 1);
  assertKnownProvider(provider);
  if (rest === "") return { provider };
  return TIER_KEYWORDS.has(rest)
    ? { provider, tier: rest as ModelTier }
    : { provider, model: rest };
}

/** Normalize a {@link ModelLike} to a plain {@link ModelSpec}. */
export function toModelSpec(model: ModelLike): ModelSpec {
  if (typeof model === "string") return parseModelString(model);
  return model instanceof Model ? model.toSpec() : model;
}

/**
 * A generic model over any OpenAI-compatible endpoint (the honest extension point for providers
 * not compiled in by name). Routes through the Rust OpenAI-compatible adapter.
 */
export class OpenAICompatibleModel extends Model {
  readonly spec: ModelSpec;
  constructor(model: string, opts?: { baseURL?: string; apiKeyEnv?: string; tier?: ModelTier }) {
    super();
    this.spec = { provider: "openai", model, baseURL: opts?.baseURL, apiKeyEnv: opts?.apiKeyEnv, tier: opts?.tier };
  }
}

/** `openaiCompatible("model", { baseURL })` — a model on a custom OpenAI-compatible server. */
export function openaiCompatible(
  model: string,
  opts?: { baseURL?: string; apiKeyEnv?: string; tier?: ModelTier }
): OpenAICompatibleModel {
  return new OpenAICompatibleModel(model, opts);
}

// ---------------------------------------------------------------------------
// The unified `model` surface (ADR 0034, phase 16d) — the DX entry point.
// ---------------------------------------------------------------------------

/** Per-call provider override (rare; the simple case needs none). */
export type ModelOptions = { baseURL?: string; apiKeyEnv?: string };

/** A provider entry: callable (`model.openai("gpt-4o")`) with tier-valued properties
 * (`model.openai.fast`). `id` is open (`string`) so a model shipped today works today;
 * typed per-provider id catalogs are an additive refinement. */
export type ProviderEntry = ((id?: string, opts?: ModelOptions) => Model) & {
  readonly fast: Model;
  readonly balanced: Model;
  readonly frontier: Model;
  readonly creative: Model;
};

function providerEntry(provider: ProviderSlug): ProviderEntry {
  const make = (id?: string, opts?: ModelOptions): Model =>
    new SpecModel({ provider, model: id, baseURL: opts?.baseURL, apiKeyEnv: opts?.apiKeyEnv });
  return Object.assign(make, {
    fast: new SpecModel({ provider, tier: "fast" }) as Model,
    balanced: new SpecModel({ provider, tier: "balanced" }) as Model,
    frontier: new SpecModel({ provider, tier: "frontier" }) as Model,
    creative: new SpecModel({ provider, tier: "creative" }) as Model
  });
}

/** The unified entry point. One import, one mental model (ADR 0034):
 *
 * ```ts
 * import { model } from "@ailu-ai/graph-sdk";
 * await model.invoke("hi");                       // zero-config: provider from env, fails loud if none
 * await model.openai("gpt-4o").invoke("hi");      // provider IS the method
 * await model.fast.invoke("classify");            // tier-only: provider from env
 * await model.anthropic.frontier.invoke("hi");    // provider + tier
 * model({ provider: "openai", tier: "fast" });    // object form (also accepts "openai:fast")
 * model.openaiCompatible({ baseURL, model });     // any OpenAI-wire endpoint
 * ```
 * `model.cohere` is a **compile error** (not a key). Unknown providers fail loud at runtime. */
export const model = Object.assign(
  (spec: ModelSpec | string): Model => new SpecModel(toModelSpec(spec)),
  {
    openai: providerEntry("openai"),
    anthropic: providerEntry("anthropic"),
    gemini: providerEntry("google"),
    mistral: providerEntry("mistral"),
    ollama: providerEntry("ollama"),
    openrouter: providerEntry("openrouter"),
    minimax: providerEntry("minimax"),
    huggingface: providerEntry("huggingface"),
    lmstudio: providerEntry("lmstudio"),
    // Provider-less tier handles (the engine / resolver picks the provider from env).
    fast: new SpecModel({ tier: "fast" }) as Model,
    balanced: new SpecModel({ tier: "balanced" }) as Model,
    frontier: new SpecModel({ tier: "frontier" }) as Model,
    creative: new SpecModel({ tier: "creative" }) as Model,
    /** Zero-config one-shot: balanced tier, provider resolved from env (fails loud if none). */
    invoke: (input: string | ChatMessage[], opts?: InvokeOptions): Promise<ModelResponse> =>
      new SpecModel({ tier: "balanced" }).invoke(input, opts),
    /** Any OpenAI-compatible endpoint (vLLM, LM Studio, a gateway, …). */
    openaiCompatible: (opts: { baseURL: string; model: string; apiKeyEnv?: string }): Model =>
      new SpecModel({
        provider: "openai",
        model: opts.model,
        baseURL: opts.baseURL,
        apiKeyEnv: opts.apiKeyEnv
      })
  }
);

/** Alias of {@link model} for teams that use `model` as a local variable name. */
export const models = model;
