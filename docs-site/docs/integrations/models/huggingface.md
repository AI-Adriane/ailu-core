---
sidebar_position: 9
title: Hugging Face
description: Run Hugging Face Inference models through Ailu's OpenAI-compatible adapter, selected by the HF_TOKEN credential in the environment.
---

# Hugging Face

Hugging Face's [Inference Router](https://router.huggingface.co/v1) is **OpenAI-compatible**, so
Ailu reaches it through the single OpenAI-compatible adapter — no dedicated integration. Set
`HF_TOKEN` and the provider becomes available; pin a model or let a `tier` resolve from the
environment.

## Config

| | |
| --- | --- |
| Wire token | `huggingface` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://router.huggingface.co/v1` |
| Credential | `HF_TOKEN` |

This is one of the OpenAI-compatible family: every member shares **one** adapter, so adding a
provider is a base URL + a key, not a new integration (ADR 0005). The native adapters —
**Anthropic** and **Google Gemini** — are the exceptions; they map each vendor's own
request/response shape. See [Providers & BYOM](/docs/building/providers) for the full table.

## Usage

Pin `provider` / `model` explicitly:

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "hf-qa" })
  .agentNode("assistant", {
    provider: "huggingface",
    model: "meta-llama/Llama-3.3-70B-Instruct", // a model id served by the HF router
    prompt: { system: "You are a concise assistant." }
  })
  .compile();

const result = await app.run({});
```

Or declare a `tier` and let `ModelPolicy` resolve it from the environment — with only `HF_TOKEN`
present, every tier maps to the Hugging Face column:

```ts
createGraph({ name: "hf-tiered" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    tier: "balanced" // env-aware; resolved to a concrete model by ModelPolicy
  })
  .compile();
```

An explicit `provider` / `model` always overrides the tier. With no credential present every
provider resolves to the deterministic mock, so the examples above run keyless.

## See also

- [Model providers](/docs/integrations/models/overview) — the full provider list.
- [Providers & BYOM](/docs/building/providers) — adapters, tiers, and env selection.
