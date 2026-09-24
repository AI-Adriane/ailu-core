/**
 * `@ailu-ai/model-mistral` — the MistralModel model overlay (ADR 0031). A thin declaration over the Rust
 * engine: pass it to `agentNode({ model })`, or call it standalone with `.invoke()`.
 */

import { Model, type ModelSpec, type ModelTier } from "@ailu-ai/model-core";

export type MistralModelOptions = { tier?: ModelTier; baseURL?: string; apiKeyEnv?: string };

/** A MistralModel model declaration. `model` wins; omit it + set a tier to let the engine resolve one. */
export class MistralModel extends Model {
  readonly spec: ModelSpec;
  constructor(model?: string, opts?: MistralModelOptions) {
    super();
    this.spec = {
      provider: "mistral",
      model,
      tier: opts?.tier,
      baseURL: opts?.baseURL,
      apiKeyEnv: opts?.apiKeyEnv
    };
  }
}

/** `mistral("...")` — plus tier shortcuts `mistral.frontier()` / `.balanced()` / `.fast()`. */
export const mistral = Object.assign(
  (model?: string, opts?: MistralModelOptions): MistralModel => new MistralModel(model, opts),
  {
    frontier: (opts?: Omit<MistralModelOptions, "tier">): MistralModel =>
      new MistralModel(undefined, { ...opts, tier: "frontier" }),
    balanced: (opts?: Omit<MistralModelOptions, "tier">): MistralModel =>
      new MistralModel(undefined, { ...opts, tier: "balanced" }),
    fast: (opts?: Omit<MistralModelOptions, "tier">): MistralModel =>
      new MistralModel(undefined, { ...opts, tier: "fast" })
  }
);
