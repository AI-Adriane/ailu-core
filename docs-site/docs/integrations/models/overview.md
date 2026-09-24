---
sidebar_position: 1
title: Models overview
description: Bring your own model — two native adapters (Anthropic, Gemini), one shared OpenAI-compatible adapter, selected by environment via ModelPolicy and capability tiers.
---

# Models overview

Ailu is **bring-your-own-model**. The engine never hardcodes a vendor: the provider that
actually runs is decided by which credential is present in the environment. Same graphs, same
code — a deployment can sit on a frontier US model, a hosted EU model, or run fully on-premise
with a local server. Only configuration changes.

Every LLM call routes through the LLM Gateway, the one seam allowed to touch a provider SDK or
HTTP API. See [The LLM Gateway](/docs/building/llm-gateway) for the adapter/port contract and
[Providers & BYOM](/docs/building/providers) for the full provider matrix.

## Two native adapters + one shared OpenAI-compatible adapter

There are exactly two adapter kinds:

- **Native** adapters map a provider's own request/response shape. Two ship:
  [Anthropic](./anthropic) (Messages API) and [Google Gemini](./google)
  (`generateContent`). Gemini is native because its API is not Chat-Completions-shaped — a
  native adapter keeps access to provider-native features instead of a lowest-common-denominator
  endpoint.
- **One** OpenAI-compatible adapter drives every other provider. Any
  server speaking the OpenAI `/v1/chat/completions` shape rides on it — adding one is a base URL
  + a key (an enum slot + a constructor), not a new integration.

| Provider | Wire token | Adapter | Endpoint | Credential |
| --- | --- | --- | --- | --- |
| [Anthropic](./anthropic) | `anthropic` | native (Messages API) | `api.anthropic.com` | `ANTHROPIC_API_KEY` |
| [Google Gemini](./google) | `google` | native (`generateContent`) | `generativelanguage.googleapis.com` | `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| [OpenAI](./openai) | `openai` | OpenAI-compatible | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| [OpenRouter](./openrouter) | `openrouter` | OpenAI-compatible | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| MiniMax | `minimax` | OpenAI-compatible | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` |
| [Hugging Face](./huggingface) | `huggingface` | OpenAI-compatible | `https://router.huggingface.co/v1` | `HF_TOKEN` |
| [Mistral](./mistral) | `mistral` | OpenAI-compatible | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| [Ollama](./ollama) (local) | `ollama` | OpenAI-compatible | `http://localhost:11434/v1` | keyless · `AILU_USE_OLLAMA=1` |
| LM Studio (local) | `lmstudio` | OpenAI-compatible | `http://localhost:1234/v1` | keyless · `AILU_USE_LMSTUDIO=1` |

:::note The native gateway ships the full set
The default execution path is the Rust `crates/llm-gateway` (reached through `@ailu-ai/napi`),
which ships every provider above. The deprecated TypeScript fallback gateway intentionally keeps
only Anthropic + the OpenAI-compatible adapter (Mistral / Ollama). The two share the same wire
shapes and the same model-policy table by design.
:::

## Selection by environment + capability tiers

Agents declare an **abstract capability tier**, not a model id. `ModelPolicy` maps a tier onto a
concrete `{ provider, model }` given the providers actually available from the environment.

| Tier | What it means |
| --- | --- |
| `frontier` | the strongest model |
| `balanced` | the everyday default |
| `fast` | cheap and quick |
| `creative` | tuned for generative writing |

`availableFromEnv()` reads the process env to decide which providers are usable; `resolve()` then
walks the cross-provider preference order (highest first) and takes the first available provider
that can serve the tier. The default preference is
`anthropic → openai → google → mistral → openrouter → minimax → huggingface → ollama → lmstudio`.

```ts
import { ModelPolicy } from "@ailu-ai/graph-sdk";

const policy = new ModelPolicy();

// Only Mistral has a key → every tier resolves to the Mistral column:
policy.resolve("frontier", ["mistral"]);
// → { provider: "mistral", model: "mistral-large-latest", recommended: true }

// Anthropic available, want the cheap tier:
policy.resolve("fast", ["anthropic"]);
// → { provider: "anthropic", model: "claude-haiku-4-5", recommended: true }

// An explicit model always wins (recommended: false):
policy.resolve("fast", ["anthropic"], { model: "claude-opus-4-8" });
// → { provider: "anthropic", model: "claude-opus-4-8", recommended: false }

// Nothing available → deterministic mock fallback:
policy.resolve("balanced", []);
// → { provider: "mock", model: "mock-model", recommended: false }
```

`recommended` is `true` only when the model came from the policy's per-tier default; an explicit
`provider` / `model` override marks it `false`. The default per-provider tier table (mirrored
byte-for-byte between the TS and Rust gateways):

| Tier | `anthropic` | `openai` | `google` | `mistral` |
| --- | --- | --- | --- | --- |
| `frontier` | `claude-opus-4-8` | `gpt-4o` | `gemini-1.5-pro` | `mistral-large-latest` |
| `balanced` | `claude-sonnet-4-6` | `gpt-4o` | `gemini-2.0-flash` | `mistral-medium-latest` |
| `fast` | `claude-haiku-4-5` | `gpt-4o-mini` | `gemini-2.0-flash` | `mistral-small-latest` |
| `creative` | `claude-fable-5` | `gpt-4o` | `gemini-2.0-flash` | `mistral-large-latest` |

The remaining OpenAI-compatible providers map every tier to a single model:
`openrouter` → `openai/gpt-4o` (frontier/creative) · `openai/gpt-4o-mini` (balanced/fast),
`minimax` → `MiniMax-Text-01`, `huggingface` → `meta-llama/Llama-3.3-70B-Instruct`,
`ollama` → `mistral`, `lmstudio` → `local-model`.

## Offline deterministic mock

With **no credential present**, every tier resolves to the deterministic **mock**
(`{ provider: "mock", model: "mock-model" }`). The mock returns scripted responses — no SDK, no
key, no network — so an agent's reasoning loop is fully replayable and every example and test
runs keyless out of the box. Swap in a real adapter only when you want live calls. See
[the gateway's offline-determinism section](/docs/building/llm-gateway).

## On-premise / sovereign

For a zero-egress deployment, set `AILU_USE_OLLAMA=1` (or `AILU_USE_LMSTUDIO=1`) and point
the engine at a local OpenAI-compatible server. No API key leaves the perimeter — both run
through the single OpenAI-compatible adapter. This is what makes a true on-premise install
possible.

## See also

- [The LLM Gateway](/docs/building/llm-gateway) — adapter/port contract, prompt registry, tier policy.
- [Providers & BYOM](/docs/building/providers) — full provider matrix and selection-by-environment.
- [Anthropic](./anthropic) · [Google Gemini](./google) · OpenAI-compatible — per-provider pages.
