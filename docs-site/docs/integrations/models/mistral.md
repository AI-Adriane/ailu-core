---
sidebar_position: 6
title: Mistral
description: Run Ailu agents on Mistral's EU-hosted models through the OpenAI-compatible adapter — pin a model or declare a tier and let the environment choose.
---

# Mistral

[Mistral](https://mistral.ai) is an **EU-hosted** model provider. Ailu talks to it through the
single OpenAI-compatible adapter — no Mistral-specific integration code.

## Config

| | |
| --- | --- |
| Wire token | `mistral` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://api.mistral.ai/v1` |
| Credential | `MISTRAL_API_KEY` |

Mistral is one of the **OpenAI-compatible family** (alongside OpenAI, OpenRouter, MiniMax,
Hugging Face, and the local Ollama / LM Studio servers): they all route through one adapter, so
adding a provider here is a base URL + a key, not a new integration (ADR 0005). Only **Anthropic**
and **Google Gemini** ship as native adapters, mapping each vendor's own request/response shape.

## Usage

Pin the provider and a concrete `model` (an explicit `model` always wins over `tier`):

```ts
import { createGraph, DefaultLLMGateway } from "@ailu/graph-sdk";

const llm = new DefaultLLMGateway(); // reads MISTRAL_API_KEY from the environment

const app = createGraph({ name: "mistral-qa" })
  .agentNode("assistant", {
    llm,
    prompt: { system: "You are a concise assistant." },
    provider: "mistral",
    model: "mistral-large-latest"
  })
  .compile();

const result = await app.run({ question: "Summarize this contract clause." });
console.log(result.channels.agentResult.reasoning);
```

Or declare a **tier** and let the environment decide. With only `MISTRAL_API_KEY` present, every
tier resolves to the Mistral column:

```ts
createGraph({ name: "mistral-tiered" })
  .agentNode("writer", {
    llm,
    prompt: { system: "Draft a short release note." },
    tier: "balanced" // resolved to a concrete Mistral model by ModelPolicy (env-aware)
  })
  .compile();
```

:::note Offline / keyless
With no `MISTRAL_API_KEY` set, the provider resolves to the deterministic **mock**, so examples
and tests run without keys. See [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) for the
`MockLLMProviderAdapter` pattern.
:::

## See also

- [Model providers overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — adapters, tiers, and selection by environment.
