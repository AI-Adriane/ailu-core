---
sidebar_position: 10
title: Ollama (local)
description: Run Ailu graphs against a local Ollama server — keyless, on-prem, zero-egress — through the shared OpenAI-compatible adapter.
---

# Ollama (local)

[Ollama](https://ollama.com) serves models on your own machine over an OpenAI-compatible API.
Ailu talks to it through the **single OpenAI-compatible adapter** (no API key, nothing
leaves the perimeter) — the same code path as any hosted provider, only the configuration
changes. This is the on-prem / zero-egress option.

## Configuration

| | |
| --- | --- |
| Wire token | `ollama` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `http://localhost:11434/v1` |
| Credential | **keyless** — enable with `AILU_USE_OLLAMA=1` |

Ollama has no API key. The engine only routes to it when `AILU_USE_OLLAMA=1` is set in
the environment, which adds `ollama` to the providers reported by
`ModelPolicy.availableFromEnv()`.

## Usage

Set the env var before launch, then pin the provider on an agent node:

```bash
export AILU_USE_OLLAMA=1
```

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

// Pin the local provider + a model you've pulled (`ollama pull llama3.1`).
const app = createGraph({ name: "on-prem-agent" })
  .channel("question", { type: "string", default: "" })
  .agentNode("assistant", {
    provider: "ollama",
    model: "llama3.1",
    prompt: { system: "You are a helpful assistant. Use tools when needed." }
  })
  .edge("__start__", "assistant")
  .edge("assistant", "__end__");
```

Pinning `provider` / `model` overrides the tier default for that node.

Prefer not to hardcode a model? Declare a **tier** instead and let the environment decide
which provider runs. With `AILU_USE_OLLAMA=1` set and no hosted credential present, the
tier resolves to the local Ollama model recommended for that tier:

```ts
const app = createGraph({ name: "tiered-agent" })
  .channel("question", { type: "string", default: "" })
  .agentNode("assistant", {
    tier: "balanced", // frontier · balanced · fast · creative
    prompt: { system: "You are a helpful assistant." }
  })
  .edge("__start__", "assistant")
  .edge("assistant", "__end__");
```

Same graph, same code — swap the credential in the environment to move between local,
hosted-EU, and frontier models. With no credential at all, every provider resolves to the
deterministic **mock**, so examples and tests run keyless.

## Related: LM Studio

[LM Studio](https://lmstudio.ai) is the other local, keyless option, served on a different
port and enabled by its own flag:

| | |
| --- | --- |
| Wire token | `lmstudio` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `http://localhost:1234/v1` |
| Credential | **keyless** — enable with `AILU_USE_LMSTUDIO=1` |

## Adapter family

Ollama and LM Studio belong to the **OpenAI-compatible family** (alongside OpenAI,
OpenRouter, Mistral, MiniMax, Hugging Face). They share **one** adapter — adding such a
provider is a base URL + a credential, not a new integration (ADR 0005). Only **Anthropic**
and **Google Gemini** ship as **native adapters**, mapping each vendor's own request/response
shape.

## See also

- [Models overview](/docs/integrations/models/overview) — every provider Ailu speaks.
- [Providers & BYOM](/docs/building/providers) — adapter contract, tier policy, on-prem deployment.
