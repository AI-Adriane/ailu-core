---
sidebar_position: 12
title: AWS Bedrock
description: Reach AWS Bedrock from Ailu through an OpenAI-compatible proxy today; a native Bedrock adapter is planned (ADR-0005-style enum slot).
---

# AWS Bedrock

Bedrock's API is **not** Chat-Completions-shaped, and Ailu has **no native Bedrock adapter
today**. Reach it now by putting an **OpenAI-compatible proxy** (e.g. LiteLLM or
bedrock-access-gateway) in front of Bedrock and pointing the shared OpenAI-compatible adapter at
it. A first-class native adapter is **planned** — an ADR-0005-style constructor plus an enum
slot, not yet shipped.

## Status

| | |
| --- | --- |
| Native adapter | **Planned** — no `bedrock` wire token yet |
| Today's path | **External seam** — OpenAI-compatible proxy in front of Bedrock |

A native adapter would map Bedrock's own request/response shape (the way
[Anthropic](/docs/integrations/models/anthropic) and Google Gemini do) rather than route through a
compatibility endpoint. Until then, Bedrock joins via the **OpenAI-compatible family**: the
*single* OpenAI-compatible adapter, a base URL + a key — no bespoke integration (ADR 0005). Only
Anthropic and Google Gemini are native adapters today; everything else shares that one.

## Config (via proxy)

Run a proxy that exposes an OpenAI-compatible `/v1` endpoint backed by your AWS credentials, then
configure Ailu to use the OpenAI-compatible provider against that proxy.

| | |
| --- | --- |
| Wire token | `openai` (the OpenAI-compatible adapter — there is no `bedrock` token) |
| Adapter | OpenAI-compatible (shared) |
| Endpoint | your proxy, e.g. `http://localhost:4000/v1` (LiteLLM) |
| Credential | `OPENAI_API_KEY` — the proxy's key; AWS auth (`AWS_*`) lives on the proxy, not in Ailu |

The proxy holds the AWS credentials (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` /
`AWS_REGION`) and translates OpenAI Chat-Completions calls into Bedrock `InvokeModel` calls.
Ailu only sees an OpenAI-compatible endpoint.

## Usage

Point the OpenAI-compatible adapter at the proxy's base URL, then pin `provider: "openai"` and the
**proxy's** model id (which maps to a Bedrock model on the proxy side). With no key present every
provider resolves to the deterministic mock, so this graph still compiles and runs offline.

```bash
# AWS auth lives on the proxy (e.g. LiteLLM), not on Ailu.
export OPENAI_API_KEY=sk-proxy-...          # the proxy's key
export OPENAI_BASE_URL=http://localhost:4000/v1   # your OpenAI-compatible proxy
```

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "bedrock-via-proxy" })
  .agentNode("assistant", {
    prompt: { system: "You are a concise assistant. Prefix your final answer with FINAL:." },
    provider: "openai",                       // shared OpenAI-compatible adapter
    model: "bedrock-claude-sonnet",           // the model id your proxy maps to a Bedrock model
    maxIterations: 2
  })
  .compile();

const result = await app.run({});
console.log(result.channels.agentResult.reasoning);
```

The `model` is the id your proxy exposes (LiteLLM model name, etc.), which it resolves to a
concrete Bedrock model. An explicit `model` always wins over a tier.

### Or declare a tier and let env pick

Skip pinning and declare a **capability tier**; `ModelPolicy` resolves it to a concrete
`{ provider, model }` from whichever credentials are present (here, the OpenAI-compatible proxy):

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
- [Providers & BYOM](/docs/building/providers) — adapter contract, the OpenAI-compatible family, and tier policy.
- [Agent nodes & ReAct](/docs/building/agent-nodes-and-react) — the `agentNode` config and tiers.
