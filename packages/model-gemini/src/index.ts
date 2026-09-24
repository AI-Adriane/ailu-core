/**
 * `@ailu-ai/model-gemini` — the GeminiModel model overlay (ADR 0031). A thin declaration over the Rust
 * engine: pass it to `agentNode({ model })`, or call it standalone with `.invoke()`.
 */

import { Model, type ModelSpec, type ModelTier } from "@ailu-ai/model-core";

export type GeminiModelOptions = { tier?: ModelTier; baseURL?: string; apiKeyEnv?: string };

/** A GeminiModel model declaration. `model` wins; omit it + set a tier to let the engine resolve one. */
export class GeminiModel extends Model {
  readonly spec: ModelSpec;
  constructor(model?: string, opts?: GeminiModelOptions) {
    super();
    this.spec = {
      provider: "google",
      model,
      tier: opts?.tier,
      baseURL: opts?.baseURL,
      apiKeyEnv: opts?.apiKeyEnv
    };
  }
}

/** `gemini("...")` — plus tier shortcuts `gemini.frontier()` / `.balanced()` / `.fast()`. */
export const gemini = Object.assign(
  (model?: string, opts?: GeminiModelOptions): GeminiModel => new GeminiModel(model, opts),
  {
    frontier: (opts?: Omit<GeminiModelOptions, "tier">): GeminiModel =>
      new GeminiModel(undefined, { ...opts, tier: "frontier" }),
    balanced: (opts?: Omit<GeminiModelOptions, "tier">): GeminiModel =>
      new GeminiModel(undefined, { ...opts, tier: "balanced" }),
    fast: (opts?: Omit<GeminiModelOptions, "tier">): GeminiModel =>
      new GeminiModel(undefined, { ...opts, tier: "fast" })
  }
);
