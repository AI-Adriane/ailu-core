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

### Restrict the variables `apiKeyEnv` may name

A graph names the variable its key is read from, so a graph you did not write can name any
variable of your process. When you run graphs for other people, set `AILU_API_KEY_ENV_ALLOWLIST`
to the variables meant to hold endpoint keys: exact names and prefixes ending in `*`, separated by
commas.

```bash
AILU_API_KEY_ENV_ALLOWLIST="AILU_ENDPOINT_*,GATEWAY_KEY"
```

An `apiKeyEnv` that matches no entry is refused before anything is read, whether the variable
exists or not. The error names the variable, never a value: `ApiKeyEnvNotAllowedError` (code
`AILU_API_KEY_ENV_NOT_ALLOWED`) from `.invoke()`, `ailu.ApiKeyEnvNotAllowedError` from Python's
`llm_complete`, and an engine error when a graph's agent names it. Set to an empty string, it
allows none. Unset, any name is read, as before. The list applies only to `apiKeyEnv`, not to a
provider's own variable such as `OPENAI_API_KEY`.
