/**
 * `@ailu-ai/model-anthropic` — the AnthropicModel model overlay (ADR 0031). A thin declaration over the Rust
 * engine: pass it to `agentNode({ model })`, or call it standalone with `.invoke()`.
 */

import { Model, type ModelSpec, type ModelTier } from "@ailu-ai/model-core";

export type AnthropicModelOptions = { tier?: ModelTier; baseURL?: string; apiKeyEnv?: string };

/** A AnthropicModel model declaration. `model` wins; omit it + set a tier to let the engine resolve one. */
export class AnthropicModel extends Model {
  readonly spec: ModelSpec;
  constructor(model?: string, opts?: AnthropicModelOptions) {
    super();
    this.spec = {
      provider: "anthropic",
      model,
      tier: opts?.tier,
      baseURL: opts?.baseURL,
      apiKeyEnv: opts?.apiKeyEnv
    };
  }
}

/** `anthropic("...")` — plus tier shortcuts `anthropic.frontier()` / `.balanced()` / `.fast()`. */
export const anthropic = Object.assign(
  (model?: string, opts?: AnthropicModelOptions): AnthropicModel => new AnthropicModel(model, opts),
  {
    frontier: (opts?: Omit<AnthropicModelOptions, "tier">): AnthropicModel =>
      new AnthropicModel(undefined, { ...opts, tier: "frontier" }),
    balanced: (opts?: Omit<AnthropicModelOptions, "tier">): AnthropicModel =>
      new AnthropicModel(undefined, { ...opts, tier: "balanced" }),
    fast: (opts?: Omit<AnthropicModelOptions, "tier">): AnthropicModel =>
      new AnthropicModel(undefined, { ...opts, tier: "fast" })
  }
);
