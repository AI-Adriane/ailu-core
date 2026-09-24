---
sidebar_position: 1
title: LLM providers
description: The LLM gateway and the providers Ailu routes to — anthropic, openai, mistral, google (Gemini), ollama — plus the mock adapter.
---

# LLM providers

Every model call in Ailu goes through the **LLM gateway** (`@ailu-ai/llm-gateway`, re-exported by
`@ailu-ai/graph-sdk`). Agents name a provider + model; the gateway routes the request to the adapter
registered for that provider. No agent or graph imports a provider SDK directly — the gateway is the only
seam that talks to model APIs, which is what keeps routing, redaction, and model policy in one place.

## Supported providers

```ts
export const LLM_PROVIDERS = ["openai", "anthropic", "mistral", "google", "ollama", "mock"] as const;
export type LLMProvider = (typeof LLM_PROVIDERS)[number];
```

`google` is **Gemini**, first-class since graph-sdk 1.11.

## The gateway

`DefaultLLMGateway` holds one adapter per provider:

- `registerAdapter(adapter)` — register an adapter (its `provider` field is the key).
- `complete(req)` — one-shot completion; routes by `req.provider`.
- `stream(req)` — token stream (same routing).

A request carries `{ provider, model, messages, tools?, temperature?, maxTokens? }`; the response carries
`{ content, toolCalls?, stopReason, usage, model, provider }`.

## Adapters

- **`AnthropicProviderAdapter`** — native Anthropic (`provider: "anthropic"`).
- **`OpenAICompatibleProviderAdapter`** — any OpenAI-compatible `/chat/completions` endpoint. Construct it
  directly (`{ provider, baseUrl, defaultModel, apiKey? }`) or via a static factory:
  - `OpenAICompatibleProviderAdapter.openai(apiKey?, model?)`
  - `OpenAICompatibleProviderAdapter.mistral(apiKey?, model?)`
  - `OpenAICompatibleProviderAdapter.ollama(baseUrl?, model?)`
  - `OpenAICompatibleProviderAdapter.google(apiKey?, model?)` — **Gemini**, via
    `https://generativelanguage.googleapis.com/v1beta/openai` (Bearer `GEMINI_API_KEY`, default model
    `gemini-2.5-flash`).
- **`MockAdapter`** — deterministic, offline; for tests and key-less local runs.

## Example

```ts
import {
  DefaultLLMGateway,
  AnthropicProviderAdapter,
  OpenAICompatibleProviderAdapter
} from "@ailu-ai/graph-sdk";

const gateway = new DefaultLLMGateway();
gateway.registerAdapter(new AnthropicProviderAdapter());
gateway.registerAdapter(OpenAICompatibleProviderAdapter.mistral(process.env.MISTRAL_API_KEY));
gateway.registerAdapter(OpenAICompatibleProviderAdapter.google(process.env.GEMINI_API_KEY)); // Gemini

const res = await gateway.complete({
  provider: "google",
  model: "gemini-2.5-flash",
  messages: [{ role: "user", content: "Summarize this changelog." }]
});
```

## Redaction

Wrap any gateway in a **`RedactingGateway`** to scrub PII/secrets from outbound text before a provider
ever sees it — the gateway boundary is where the data plane stays clean.

## Prompts & model policy

Agents reference prompts by id/version through the `PromptRegistry` (never hardcoded), and a model policy
can map a tier (e.g. `frontier` / `fast`) to a concrete provider+model so graphs stay portable across the
providers a deployment has configured.
