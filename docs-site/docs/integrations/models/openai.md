---
sidebar_position: 4
title: OpenAI
description: Run Ailu agent nodes on OpenAI through the shared OpenAI-compatible adapter — selected by the OPENAI_API_KEY in your environment.
---

# OpenAI

OpenAI rides Ailu's **OpenAI-compatible adapter**: present `OPENAI_API_KEY` in the environment
and the engine speaks Chat Completions at `https://api.openai.com/v1` — no vendor code in your graph.

## Config

| | |
| --- | --- |
| Wire token | `openai` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://api.openai.com/v1` |
| Credential | `OPENAI_API_KEY` |

## Usage

Pin OpenAI explicitly on an `agentNode`, or declare a **tier** and let `ModelPolicy` resolve it
from the environment (with only `OPENAI_API_KEY` present, every tier maps to the OpenAI column).

```ts
import { createGraph } from "@ailu/graph-sdk";

// Pin provider + model — explicit always wins over tier.
const pinned = createGraph({ name: "openai-pinned" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant." },
    provider: "openai",
    model: "gpt-4o"
  })
  .compile();

// Or declare a tier and resolve from env (OPENAI_API_KEY → OpenAI).
const tiered = createGraph({ name: "openai-tiered" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant." },
    tier: "frontier"
  })
  .compile();
```

With no credential present the provider resolves to the deterministic **mock**, so the same graph
runs keyless in examples and tests. See [agent nodes](/docs/building/agent-nodes-and-react) for the
full `agentNode` config and an offline `MockLLMProviderAdapter` setup.

## OpenAI-compatible family

OpenAI and every other OpenAI-compatible provider (OpenRouter, Mistral, MiniMax, Hugging Face,
Ollama, LM Studio) share **one** adapter — adding a provider of that family is a base URL + a key,
not a new integration (ADR 0005). Anthropic and Google Gemini are the exceptions: each is a
**native** adapter mapping the provider's own request/response shape.

## See also

- [Models overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — wire tokens, endpoints, credentials, and tier policy.
