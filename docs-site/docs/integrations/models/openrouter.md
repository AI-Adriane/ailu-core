---
sidebar_position: 7
title: OpenRouter
description: Route Ailu agents over OpenRouter's many models (incl. free-tier) through the single OpenAI-compatible adapter.
---

# OpenRouter

OpenRouter is a **router over many models** (frontier, hosted, and free-tier) behind one
OpenAI-compatible endpoint. Ailu reaches it through the shared OpenAI-compatible adapter — no
new integration, just a base URL and a key.

## Config

| | |
| --- | --- |
| Wire token | `openrouter` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://openrouter.ai/api/v1` |
| Credential | `OPENROUTER_API_KEY` |

OpenRouter is one of the **OpenAI-compatible family**: Ailu routes it through the *single*
OpenAI-compatible adapter, so adding it is a base URL + a key, not a bespoke integration
(ADR 0005). Only **Anthropic** and **Google Gemini** are native adapters — everything else,
OpenRouter included, shares this one. See [Models overview](/docs/integrations/models/overview)
and [Providers & BYOM](/docs/building/providers).

## Usage

Pin the provider and a concrete OpenRouter model on an `agentNode`. With
`OPENROUTER_API_KEY` set in the environment, the gateway routes the call to
`https://openrouter.ai/api/v1`; with no key present every provider resolves to the deterministic
mock, so this graph still compiles and runs offline.

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "openrouter-qa" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant. Prefix your final answer with FINAL:." },
    provider: "openrouter",
    model: "openai/gpt-4o-mini", // any model id from openrouter.ai/models
    maxIterations: 2
  })
  .compile();

const result = await app.run({});
console.log(result.channels.agentResult.reasoning);
```

The `model` is the OpenRouter model id (e.g. `openai/gpt-4o-mini`, `meta-llama/llama-3.1-8b-instruct:free`).
An explicit `model` always wins over a tier.

### Or declare a tier and let env pick

Skip pinning and declare a **capability tier**; `ModelPolicy` resolves it to a concrete
`{ provider, model }` from whichever credentials are present:

```ts
createGraph({ name: "tiered" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    tier: "balanced" // resolved env-aware by ModelPolicy
  })
  .compile();
```

## See also

- [Models overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — adapter contract and tier policy.
- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) — the `agentNode` config and tiers.
