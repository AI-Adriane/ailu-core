---
sidebar_position: 11
title: NVIDIA NIM
description: Run Ailu agents on NVIDIA NIM microservices through the OpenAI-compatible adapter — hosted at integrate.api.nvidia.com or a self-hosted NIM, selected by NVIDIA_API_KEY.
---

# NVIDIA NIM

[NVIDIA NIM](https://www.nvidia.com/en-us/ai/) serves OpenAI-compatible chat completions — hosted
on NVIDIA's build platform or self-hosted on your own GPUs. Ailu reaches it through the **single
OpenAI-compatible adapter**: a base URL override plus a key, no NVIDIA-specific integration code
(ADR 0005). Set `NVIDIA_API_KEY` and point the OpenAI-compatible base URL at a NIM endpoint.

## Config

| Property | Value |
| --- | --- |
| Wire token | `openai` (shared OpenAI-compatible adapter, base URL pointed at NIM) |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://integrate.api.nvidia.com/v1` · or a self-hosted NIM |
| Credential | `NVIDIA_API_KEY` |

NVIDIA NIM is part of the **OpenAI-compatible family** (alongside OpenAI, OpenRouter, MiniMax,
Hugging Face, Mistral, and the local Ollama / LM Studio servers): they all route through one
adapter, so reaching NIM is a base URL + a key, not a new integration (ADR 0005). Only **Anthropic**
and **Google Gemini** ship as native adapters, mapping each vendor's own request/response shape.

:::note Configured via the base URL override
Ailu does not ship a dedicated `nvidia` wire token. NIM is reached by pointing the
OpenAI-compatible adapter's base URL at `https://integrate.api.nvidia.com/v1` (or your self-hosted
NIM) and supplying `NVIDIA_API_KEY`. A first-class `nvidia` selector in `ModelPolicy` is **Planned**
— for now NIM is the canonical example of the OpenAI-compatible base URL override (an **external
seam**, not bespoke code).
:::

## Usage

Pin `provider` / `model` on an `agentNode` to force the OpenAI-compatible path, with the base URL
pointed at NIM:

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "nvidia-nim-agent" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant. Prefix your final answer with FINAL:." },
    provider: "openai", // shared OpenAI-compatible adapter, base URL -> NIM
    model: "meta/llama-3.3-70b-instruct", // any model id served by the NIM endpoint
    maxIterations: 2
  })
  .compile();

const result = await app.run({});
console.log(result.status); // "completed" once NVIDIA_API_KEY + the NIM base URL are set
```

The `model` is whatever model id the NIM endpoint serves (e.g. `meta/llama-3.3-70b-instruct`,
`nvidia/llama-3.1-nemotron-70b-instruct`). An explicit `model` always wins over a `tier`.

Or declare a **tier** and let `ModelPolicy` resolve it from the environment — the OpenAI-compatible
path routes through the configured NIM base URL, so the same graph runs unchanged:

```ts
import { createGraph } from "@ailu/graph-sdk";

createGraph({ name: "tiered" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    tier: "balanced" // resolved to a concrete model by ModelPolicy (env-aware)
  })
  .compile();
```

:::note Offline / keyless
With no `NVIDIA_API_KEY` set, the provider resolves to the deterministic **mock**, so examples and
tests run without keys.
:::

## See also

- [Models overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — adapters, tiers, and selection by environment.
- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) — the `agentNode` config and tiers.
