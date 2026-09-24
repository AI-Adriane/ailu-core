---
title: Add a real agent
description: Rung 2 of the ladder — turn a plain node into a thinking LLM agent in about 30 seconds, in any model.
---

# Add a real agent

[Quickstart](./quickstart) ran a graph that **suspended on a human gate**. Its nodes were plain
functions, though — nothing actually *reasoned*. Let's make a node think: swap it for an **LLM
agent** that reads the conversation, decides, and replies. About 30 seconds.

## 1. Give Ailu a model key

The agent routes through whatever provider key is in your environment — no code change to switch
models. Set **one**:

```bash
export ANTHROPIC_API_KEY=sk-ant-...     # Claude
# or OPENAI_API_KEY=sk-...              # GPT
# or MISTRAL_API_KEY=...                # Mistral (EU)
# or GEMINI_API_KEY=...                 # Gemini
```

## 2. Add the agent

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "assistant" })
  .messagesChannel()                          // an append-reduced conversation channel
  .agentNode("assistant", {
    llm: new DefaultLLMGateway(),             // routes to the provider key in your env
    prompt: { system: "You are a concise, helpful assistant." },
    tier: "balanced"                          // a capability tier, not a hardcoded model
  })
  .compile();

const result = await app.run({
  messages: [{ role: "user", content: "Name three EU-sovereign cloud providers." }]
});

console.log(result.status);                           // "completed"
console.log(result.channels.messages.at(-1)?.content); // the agent's answer
```

That's the whole change from Quickstart: one `.agentNode(...)` instead of a plain `.node(...)`.

## Tiers, not hardcoded models

You asked for a **tier** (`"frontier" | "balanced" | "fast" | "creative"`), not a model string.
Ailu resolves it against the keys you set, so the same graph runs on Claude, GPT, Mistral, or
Gemini with **zero code change** — set a different env var and rerun.

| You set | `balanced` resolves to |
| --- | --- |
| `ANTHROPIC_API_KEY` | a current Claude model |
| `OPENAI_API_KEY` | a current GPT model |
| `MISTRAL_API_KEY` | a current Mistral model (EU) |
| `GEMINI_API_KEY` | a current Gemini model |

Need an exact model? Pin it — an explicit `model` always wins over `tier`:

```ts
.agentNode("assistant", {
  llm: new DefaultLLMGateway(),
  provider: "anthropic",
  model: "claude-sonnet-4-6"
})
```

:::tip No key? Run it offline
For CI or a no-key demo, pass a `MockLLMProviderAdapter` with a scripted reply instead of a real
key — the graph is identical. See [Agent nodes & ReAct](/docs/building/agent-nodes-and-react).
:::

## What you just did

You upgraded a structural graph into an **agentic** one: a node that reasons over state and writes
back into a channel — still deterministic to **resume** and **checkpoint**, because the agent runs
*inside* the same governed runtime.

## Next — govern it

Your agent can think. The moment it reaches for something sensitive (a refund, a delete, an email),
you want a human in the loop **and** cryptographic proof of the decision.

➡️ **[Governance quickstart](./governance-quickstart)** — gate a tool, approve it, and get a signed,
replayable attestation. The third and final rung.
