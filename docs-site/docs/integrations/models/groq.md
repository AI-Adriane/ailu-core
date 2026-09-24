---
sidebar_position: 8
title: Groq
description: Fast inference via the OpenAI-compatible adapter pointed at api.groq.com, selected by GROQ_API_KEY.
---

# Groq

[Groq](https://groq.com) serves OpenAI-compatible chat completions over its LPU inference stack.
Ailu reaches it through the **single OpenAI-compatible adapter** — a base URL override plus a
key, no new integration code (ADR 0005). Set `GROQ_API_KEY` and you have fast inference behind the
same graphs.

## Config

| Property | Value |
| --- | --- |
| Wire token | `groq` |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | `https://api.groq.com/openai/v1` |
| Credential | `GROQ_API_KEY` |

Native vs OpenAI-compatible: **Anthropic** and **Google Gemini** ship as native adapters (the
provider's own request/response shape). Every other provider — Groq included — shares one
OpenAI-compatible adapter, so adding one is a base URL + a key, not a new client.

## Usage

Pin `provider` / `model` on an `agentNode` to force Groq explicitly:

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "groq-agent" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant. Prefix your final answer with FINAL:." },
    provider: "groq",
    model: "llama-3.3-70b-versatile",
    maxIterations: 2
  })
  .compile();

const result = await app.run({});
console.log(result.status); // "completed" once GROQ_API_KEY is set
```

Or declare a **tier** and let `ModelPolicy` resolve it from the environment — with `GROQ_API_KEY`
present, the OpenAI-compatible path routes through Groq, so the same graph runs unchanged:

```ts
import { createGraph } from "@ailu/graph-sdk";

createGraph({ name: "tiered" })
  .agentNode("writer", {
    prompt: { system: "Draft a short release note." },
    tier: "fast" // resolved to a concrete model by ModelPolicy (env-aware)
  })
  .compile();
```

An explicit `model` always wins over `tier`. With no credential present every provider resolves to
the deterministic mock, so examples and tests run keyless.

## See also

- [Models overview](/docs/integrations/models/overview) — the full provider matrix.
- [Providers & BYOM](/docs/building/providers) — adapters, tiers, and selection by environment.
