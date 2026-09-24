---
sidebar_position: 8
title: Providers & BYOM
description: The LLM providers Ailu speaks — native Anthropic & Gemini plus the OpenAI-compatible family — selected by environment, so you bring your own model.
---

# Providers & BYOM

Ailu is **bring-your-own-model**. The engine never hardcodes a vendor; the provider that
actually runs is chosen by which credential is present in the environment. A deployment can
sit on a hosted EU model, on a frontier US model, or run **fully on-premise** with a local
model — same graphs, same code, only configuration changes.

All provider calls go through the LLM Gateway (see [The LLM Gateway](/docs/building/llm-gateway)).
On the default **Rust** execution path the gateway ships the full provider set below; the
deprecated TypeScript fallback gateway intentionally keeps only Anthropic + the
OpenAI-compatible adapter.

## Supported providers

| Provider | Wire token | Adapter | Endpoint | Credential |
| --- | --- | --- | --- | --- |
| Anthropic | `anthropic` | native (Messages API) | `api.anthropic.com` | `ANTHROPIC_API_KEY` |
| Google Gemini | `google` | **native** (`generateContent`) | `generativelanguage.googleapis.com` | `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| OpenAI | `openai` | OpenAI-compatible | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| OpenRouter | `openrouter` | OpenAI-compatible | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| MiniMax | `minimax` | OpenAI-compatible | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` |
| Hugging Face | `huggingface` | OpenAI-compatible | `https://router.huggingface.co/v1` | `HF_TOKEN` |
| Mistral | `mistral` | OpenAI-compatible | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| Ollama (local) | `ollama` | OpenAI-compatible | `http://localhost:11434/v1` | keyless · `AILU_USE_OLLAMA=1` |
| LM Studio (local) | `lmstudio` | OpenAI-compatible | `http://localhost:1234/v1` | keyless · `AILU_USE_LMSTUDIO=1` |

Two adapter kinds: **native** ones (Anthropic, Gemini) map the provider's own request/response
shape; everything else shares **one** OpenAI-compatible adapter — adding a provider of that
family is a constructor + an enum slot, not a new integration (ADR 0005, in the repo's `docs/adr/`).

:::note Why Gemini is native
Google's `generateContent` API is not Chat-Completions-shaped. A native adapter (mirroring the
Anthropic one) keeps access to Gemini-native request features rather than routing through a
lowest-common-denominator compatibility endpoint.
:::

## Selection by environment

`ModelPolicy.availableFromEnv()` reports which providers are usable from the present credentials.
The capability tier then resolves to a concrete `{ provider, model }`:

- **Tiers** — `frontier`, `balanced`, `fast`, `creative` (an abstract capability level, not a
  model id). An agent node declares a tier; the policy maps it to the highest-preference
  available provider's recommended model for that tier.
- **Override** — a request (or a per-base pin) may force a specific `provider` / `model`,
  overriding the tier default.
- **Offline** — with no credential present, every provider resolves to the deterministic
  **mock**, so examples and tests run keyless.

## On-premise / sovereign

For a zero-egress deployment, set `AILU_USE_OLLAMA=1` (or `AILU_USE_LMSTUDIO=1`) and point
the engine at a local OpenAI-compatible server. No API key leaves the perimeter — embeddings and
chat both run locally. This is what makes a true on-premise install possible (see the sovereign
deployment ADR).

## See also

- [The LLM Gateway](/docs/building/llm-gateway) — the adapter/port contract and the tier policy.
- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) — how an agent declares a tier.
