---
sidebar_position: 5
title: Microsoft Azure OpenAI
description: Run Ailu agents against an Azure OpenAI deployment via the OpenAI-compatible adapter — base URL override + api-key, deployment names as model ids.
---

# Microsoft Azure OpenAI

Azure OpenAI is not a separate adapter — it is the **OpenAI-compatible adapter** pointed at an
Azure deployment endpoint with the `api-key` credential. Adding it is a base URL + a key, not a
new integration (ADR 0005).

## Configuration

| Wire token | Adapter | Endpoint | Credential |
| --- | --- | --- | --- |
| `openai` (OpenAI-compatible) | OpenAI-compatible (base URL override) | `https://<resource>.openai.azure.com/openai/deployments/<deployment>` | `api-key` (`OPENAI_API_KEY`) |

:::note Deployment names are model ids
On Azure the `model` field is your **deployment name**, not a catalog model id like `gpt-4o`. Pin
the deployment you created in the Azure portal.
:::

## Usage

Point the OpenAI-compatible adapter at the deployment endpoint and pin `provider` / `model`
(the deployment name) on the agent node:

```ts
import {
  createGraph,
  DefaultLLMGateway,
  OpenAICompatibleProviderAdapter,
  type LLMGateway
} from "@ailu-ai/graph-sdk";

const azure = (): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new OpenAICompatibleProviderAdapter({
      provider: "openai",
      apiKey: process.env.OPENAI_API_KEY,
      baseUrl:
        "https://<resource>.openai.azure.com/openai/deployments/<deployment>"
    })
  );
  return gateway;
};

const app = createGraph({ name: "azure-qa" })
  .agentNode("assistant", {
    llm: azure(),
    prompt: { system: "You are a concise assistant." },
    provider: "openai",
    model: "<deployment>" // Azure deployment name, used as the model id
  })
  .compile();

const result = await app.run({});
```

Prefer a capability **tier** and let the engine resolve the concrete model from the environment
instead of pinning a deployment inline:

```ts
createGraph({ name: "azure-tiered" })
  .agentNode("writer", {
    llm: azure(),
    prompt: { system: "Draft a short release note." },
    tier: "balanced" // resolved by ModelPolicy against available credentials
  })
  .compile();
```

An explicit `model` always overrides the `tier`. With no credential present the provider resolves
to the deterministic mock, so examples run keyless.

## Adapter family

Azure shares the **single OpenAI-compatible adapter** with OpenAI, OpenRouter, MiniMax, Hugging
Face, Mistral, Ollama, and LM Studio — they differ only by base URL and key (ADR 0005). The
**native** adapters are Anthropic (Messages API) and Google Gemini (`generateContent`); those map
the provider's own request/response shape rather than the Chat-Completions wire.

## See also

- [Model integrations overview](/docs/integrations/models/overview)
- [Providers & BYOM](/docs/building/providers) — the full provider table and tier policy.
