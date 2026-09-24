---
sidebar_position: 3
title: Google Gemini
description: Run Ailu agents on Google Gemini through the native generateContent adapter, selected by GEMINI_API_KEY (or GOOGLE_API_KEY) in the environment.
---

# Google Gemini

Google Gemini is a **native** provider in Ailu — not OpenAI-compatible. Its `generateContent`
API isn't Chat-Completions-shaped, so it gets its own adapter (mirroring Anthropic) rather than
routing through the shared compatibility endpoint. Set a credential in the environment and Gemini
becomes selectable by tier; the graph and code don't change (Ailu is
[bring-your-own-model](/docs/building/providers)).

## Configuration

| | |
| --- | --- |
| Wire token | `google` |
| Adapter | **native** (`generateContent`) |
| Endpoint | `generativelanguage.googleapis.com` |
| Credential | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |

Native vs. OpenAI-compatible: Anthropic and Google each map the provider's own request/response
shape. Every other provider (OpenAI, OpenRouter, Mistral, Ollama, …) shares **one**
OpenAI-compatible adapter — so adding one of those is just a base URL + a key, not a new
integration (ADR 0005). Gemini is native to keep access to Gemini-native request features.

## Usage

An agent node consumes an `LLMGateway` and declares either a capability **tier** (resolved against
the credentials present in the environment) or a pinned `provider` / `model`. With only
`GEMINI_API_KEY` set, every tier resolves to the Gemini column.

```ts
import { createGraph } from "@ailu/graph-sdk";

// GEMINI_API_KEY (or GOOGLE_API_KEY) present in the environment.

// Option A — declare a tier, let ModelPolicy resolve it to a Gemini model:
const tiered = createGraph({ name: "gemini-tiered" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    tier: "balanced"
  })
  .compile();

// Option B — pin the provider explicitly (an explicit model always wins over tier):
const pinned = createGraph({ name: "gemini-pinned" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    provider: "google",
    model: "gemini-2.5-pro"
  })
  .compile();

const result = await pinned.run({});
console.log(result.status);                         // "completed"
console.log(result.channels.agentResult.reasoning); // ReAct trace
```

With **no** credential present, every provider — Gemini included — resolves to the deterministic
mock, so examples and tests run keyless. To run against the live API, just export
`GEMINI_API_KEY` (or `GOOGLE_API_KEY`).

## See also

- [Model integrations overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — adapters, tiers, and selection by environment.
