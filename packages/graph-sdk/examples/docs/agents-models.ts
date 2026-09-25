import assert from "node:assert/strict";

// #region example
import { model } from "@ailu-ai/graph-sdk";

// A provider and a model id. The key comes from the provider's variable (OPENAI_API_KEY, ...).
const gpt = model.openai("gpt-4o");
const claude = model.anthropic("claude-sonnet-4-6");

// A capability tier: the provider is picked from the keys you have set.
const cheap = model.fast;
const careful = model.frontier;

// A provider and a tier.
const mistralFast = model.mistral.fast;

// The same thing as a string, handy in config files.
const fromConfig = model("openai:gpt-4o");

// Any OpenAI-compatible server: vLLM, LM Studio, a gateway. The key is read only from apiKeyEnv.
const local = model.openaiCompatible({
  baseURL: "http://localhost:8000/v1",
  model: "llama-3.1-8b-instruct",
  apiKeyEnv: "MY_GATEWAY_KEY"
});
// #endregion example

assert.equal(gpt.toSpec().provider, "openai");
assert.equal(claude.toSpec().model, "claude-sonnet-4-6");
assert.equal(cheap.toSpec().tier, "fast");
assert.equal(careful.toSpec().tier, "frontier");
assert.equal(mistralFast.toSpec().provider, "mistral");
assert.equal(fromConfig.toSpec().model, "gpt-4o");
assert.equal(local.toSpec().baseURL, "http://localhost:8000/v1");
