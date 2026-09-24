import type { LLMProvider } from "./types.js";

/**
 * An abstract capability tier. Wire-compatible (camelCase) with the Rust
 * `ModelTier` enum in `crates/llm-gateway`.
 */
export type ModelTier = "frontier" | "balanced" | "fast" | "creative";

/** All four tiers, in declaration order — handy for seeding tables. */
export const MODEL_TIERS: readonly ModelTier[] = ["frontier", "balanced", "fast", "creative"];

/**
 * The outcome of {@link ModelPolicy.resolve}: a concrete provider + model, plus
 * whether the model came from the recommended per-tier defaults (`true`) or from
 * an explicit override (`false`).
 */
export type ModelChoice = {
  provider: LLMProvider;
  model: string;
  recommended: boolean;
};

/** `tier -> model` for a single provider. */
export type TierModelTable = Record<ModelTier, string>;

/** Optional override passed to {@link ModelPolicy.resolve}. */
export type ResolveOverride = {
  provider?: LLMProvider;
  model?: string;
};

/** The model fallback used when no provider is available at all. */
export const MOCK_MODEL = "mock-model";

/**
 * The shared capability-tier contract defaults: the recommended model for each
 * provider at each tier. Mirrors the Rust `ModelPolicy::default` table exactly.
 */
export const DEFAULT_TIER_TABLE: Partial<Record<LLMProvider, TierModelTable>> = {
  anthropic: {
    frontier: "claude-opus-4-8",
    balanced: "claude-sonnet-4-6",
    fast: "claude-haiku-4-5",
    creative: "claude-fable-5"
  },
  mistral: {
    frontier: "mistral-large-latest",
    balanced: "mistral-medium-latest",
    fast: "mistral-small-latest",
    creative: "mistral-large-latest"
  },
  ollama: {
    frontier: "mistral",
    balanced: "mistral",
    fast: "mistral",
    creative: "mistral"
  }
};

/** The default cross-provider preference order, highest first. */
export const DEFAULT_PREFERENCE: readonly LLMProvider[] = ["anthropic", "mistral", "ollama"];

/**
 * Capability-tier model policy: map an abstract capability tier
 * (`frontier` / `balanced` / `fast` / `creative`) onto a concrete
 * `{ provider, model }` choice, given which providers are actually available.
 *
 * Mirrors the Rust `crates/llm-gateway` `ModelPolicy` byte for byte in behaviour
 * and wire shape. The point: "I only have Mistral" -> every tier resolves to the
 * mistral column; "only Anthropic" -> `fast` -> haiku, `frontier` -> opus,
 * `creative` -> fable.
 */
export class ModelPolicy {
  private readonly table: Partial<Record<LLMProvider, TierModelTable>>;
  private readonly preference: readonly LLMProvider[];

  /**
   * Construct a policy. Either argument may be omitted to keep the contract
   * default for that piece.
   */
  public constructor(options?: {
    table?: Partial<Record<LLMProvider, TierModelTable>>;
    preference?: readonly LLMProvider[];
  }) {
    this.table = options?.table ?? DEFAULT_TIER_TABLE;
    this.preference = options?.preference ?? DEFAULT_PREFERENCE;
  }

  /**
   * Which providers are usable given the current process environment:
   * `anthropic` iff `ANTHROPIC_API_KEY` is set; `mistral` iff `MISTRAL_API_KEY`
   * is set; `ollama` iff `AILU_USE_OLLAMA=1`. Order follows the policy
   * preference so callers get a deterministic list.
   */
  public availableFromEnv(env: NodeJS.ProcessEnv = process.env): LLMProvider[] {
    const anthropic = isPresent(env.ANTHROPIC_API_KEY);
    const mistral = isPresent(env.MISTRAL_API_KEY);
    const ollama = env.AILU_USE_OLLAMA === "1";

    return this.preference.filter((p) => {
      if (p === "anthropic") return anthropic;
      if (p === "mistral") return mistral;
      if (p === "ollama") return ollama;
      return false;
    });
  }

  /**
   * Resolve a capability tier to a concrete `{ provider, model, recommended }`.
   *
   * - An explicit `override.provider` and/or `override.model` wins, with
   *   `recommended = false`. When only one of the two is given, the other is
   *   filled from the policy: an override provider maps the tier to that
   *   provider's recommended model; an override model rides on the first
   *   available provider (or the override provider if also given).
   * - Otherwise the highest-preference provider that is both available and
   *   present in the table supplies its tier model, with `recommended = true`.
   * - If nothing is available, the mock provider is returned.
   */
  public resolve(
    tier: ModelTier,
    available: readonly LLMProvider[],
    override?: ResolveOverride
  ): ModelChoice {
    const overrideProvider = override?.provider;
    const overrideModel = override?.model;

    // An explicit override (provider and/or model) takes precedence and is never
    // flagged as a recommended default.
    if (overrideProvider !== undefined || overrideModel !== undefined) {
      const provider = overrideProvider ?? this.firstAvailable(available) ?? "mock";
      const model = overrideModel ?? this.modelFor(provider, tier) ?? MOCK_MODEL;
      return { provider, model, recommended: false };
    }

    // No override: walk the preference order and take the first available
    // provider that can serve this tier.
    for (const provider of this.preference) {
      if (available.includes(provider)) {
        const model = this.modelFor(provider, tier);
        if (model !== undefined) {
          return { provider, model, recommended: true };
        }
      }
    }

    // Nothing available -> mock.
    return { provider: "mock", model: MOCK_MODEL, recommended: false };
  }

  /** The recommended model for a provider+tier from the table, if present. */
  private modelFor(provider: LLMProvider, tier: ModelTier): string | undefined {
    return this.table[provider]?.[tier];
  }

  /** The first preference-ordered provider that is in `available`. */
  private firstAvailable(available: readonly LLMProvider[]): LLMProvider | undefined {
    return this.preference.find((p) => available.includes(p));
  }
}

function isPresent(value: string | undefined): boolean {
  return value !== undefined && value !== "";
}
