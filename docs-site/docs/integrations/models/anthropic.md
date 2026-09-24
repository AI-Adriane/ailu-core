---
sidebar_position: 2
title: Anthropic
description: Run agent nodes on Anthropic's Claude models through Ailu's native Messages-API adapter — selected by ANTHROPIC_API_KEY in the environment.
---

# Anthropic

Ailu talks to Claude through a **native** adapter that maps the Anthropic
[Messages API](https://api.anthropic.com) request/response shape directly — no
lowest-common-denominator translation. It runs when `ANTHROPIC_API_KEY` is present in the
environment, or as the deterministic mock when it isn't.

## Config

| | |
| --- | --- |
| Wire token | `anthropic` |
| Adapter | native (Messages API) |
| Endpoint | `api.anthropic.com` |
| Credential | `ANTHROPIC_API_KEY` |

A **native** adapter maps the provider's own request/response shape, keeping access to
Anthropic-native request features rather than routing through a compatibility endpoint. The
OpenAI-compatible providers instead share **one** adapter — adding one is a base URL + key, not
a new integration (ADR 0005). Anthropic and [Google Gemini](/docs/integrations/models/overview)
are the two native adapters.

## Usage

Set the credential in the environment, then either pin `provider`/`model` on an agent node, or
declare a `tier` and let `ModelPolicy` resolve it per-environment.

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

```ts
import { createGraph } from "@ailu/graph-sdk";

const app = createGraph({ name: "summarize" })
  // Pin Anthropic + a concrete Claude model.
  .agentNode("draft", {
    llm,
    prompt: { system: "Summarize the input in three bullets." },
    provider: "anthropic",
    model: "claude-sonnet-4-6"
  })
  // Or declare a capability tier and let ModelPolicy pick the model per-env.
  .agentNode("review", {
    llm,
    prompt: { system: "Critique the draft for accuracy." },
    tier: "frontier" // resolves to an Anthropic model when ANTHROPIC_API_KEY is the available credential
  })
  .compile();
```

`model` always wins over `tier`. With no credential present, both nodes resolve to the
deterministic mock, so the graph compiles and runs keyless. The `llm` gateway is the
`LLMGateway` the agent runs on — see [agent nodes & ReAct](/docs/building/agent-nodes-and-react)
for how to construct one.

## See also

- [Model integrations overview](/docs/integrations/models/overview)
- [Providers & BYOM](/docs/building/providers) — the full provider table and tier policy.
