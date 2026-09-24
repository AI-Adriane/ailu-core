/**
 * `@ailu/model-openai` — the OpenAI model overlay (ADR 0031). A thin declaration over the
 * Rust engine: pass it to `agentNode({ model })`, or call it standalone with `.invoke()`.
 *
 * ```ts
 * import { openai, OpenAIModel } from "@ailu/model-openai";
 * const m = new OpenAIModel("gpt-4o");      // or openai("gpt-4o") / openai.frontier()
 * await m.invoke("hello");                   // one-shot via the Rust gateway
 * ```
 */

import { Model, type ModelSpec, type ModelTier } from "@ailu/model-core";

export type OpenAIOptions = { tier?: ModelTier; baseURL?: string; apiKeyEnv?: string };

/** An OpenAI model declaration. `model` wins; omit it + set a tier to let the engine resolve one. */
export class OpenAIModel extends Model {
  readonly spec: ModelSpec;
  constructor(model?: string, opts?: OpenAIOptions) {
    super();
    this.spec = {
      provider: "openai",
      model,
      tier: opts?.tier,
      baseURL: opts?.baseURL,
      apiKeyEnv: opts?.apiKeyEnv
    };
  }
}

/** `openai("gpt-4o")` — plus tier shortcuts `openai.frontier()` / `.balanced()` / `.fast()`. */
export const openai = Object.assign(
  (model?: string, opts?: OpenAIOptions): OpenAIModel => new OpenAIModel(model, opts),
  {
    frontier: (opts?: Omit<OpenAIOptions, "tier">): OpenAIModel =>
      new OpenAIModel(undefined, { ...opts, tier: "frontier" }),
    balanced: (opts?: Omit<OpenAIOptions, "tier">): OpenAIModel =>
      new OpenAIModel(undefined, { ...opts, tier: "balanced" }),
    fast: (opts?: Omit<OpenAIOptions, "tier">): OpenAIModel =>
      new OpenAIModel(undefined, { ...opts, tier: "fast" })
  }
);
