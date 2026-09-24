---
title: "Models and providers"
description: "Every provider, the variable its key is read from, the model each tier picks, and custom endpoints."
---

import ModelTiers from "./_model-tiers.md";

# Models and providers

## Pick a model

| Form | Example | Provider | Model |
| --- | --- | --- | --- |
| Provider and model id | `model.openai("gpt-4o")` | named | named |
| Provider and tier | `model.mistral.fast` | named | from the table below |
| Tier only | `model.balanced` | the first provider with a key, in the order of the table | from the table |
| String | `model("anthropic:claude-sonnet-4-6")`, `model("openai:fast")` | named | named, or from the table |
| Custom endpoint | `model.openaiCompatible({ baseURL, model, apiKeyEnv })` | any OpenAI-compatible server | named |

The same forms work for `agentNode({ model })` and for one-off calls with `.invoke()`. A string
also works directly: `agentNode({ model: "openai:gpt-4o" })`. An unknown provider is an error.

## Providers, keys and tiers

<ModelTiers />

A missing key fails the call with the name of the variable to set. `AILU_LLM_MOCK=1` turns
missing keys into calls to the deterministic offline mock instead.

Local servers need no key but must be switched on: `AILU_USE_OLLAMA=1` (with
`AILU_OLLAMA_BASE_URL` for a server not on `http://localhost:11434/v1`), `AILU_USE_LMSTUDIO=1`
(with `AILU_LMSTUDIO_BASE_URL`).

## Custom endpoints

`model.openaiCompatible` points an agent at any server that speaks the OpenAI chat-completions
API: vLLM, LM Studio, LiteLLM, Azure OpenAI, a company gateway.

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=custom-endpoint
```

The key is read only from `apiKeyEnv`. Your `OPENAI_API_KEY` is never sent to a custom endpoint.
Anthropic and Gemini have their own APIs and can't be pointed at a custom URL.
