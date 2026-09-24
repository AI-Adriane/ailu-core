---
sidebar_position: 2
title: Agent nodes & ReAct
description: Add an LLM-driven ReAct agent to a graph, and use the prebuilt micro-agents.
---

# Agent nodes & ReAct

`agentNode` adds a node backed by a **ReAct agent**: it reasons, optionally calls tools, and
writes its `AgentResult` into an output channel (default `"agentResult"`). You drive it with an
`LLMGateway`.

Offline, use a `DefaultLLMGateway` plus a `MockLLMProviderAdapter` with a scripted response —
no API key, fully deterministic.

```ts
import {
  createGraph,
  DefaultLLMGateway,
  MockLLMProviderAdapter,
  type LLMGateway
} from "@ailu-ai/graph-sdk";

const mockLLM = (): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(
    new MockLLMProviderAdapter({
      provider: "anthropic",
      response: {
        content: "FINAL: The capital of France is Paris.",
        usage: { promptTokens: 0, completionTokens: 0 },
        model: "mock",
        provider: "anthropic"
      }
    })
  );
  return gateway;
};

const app = createGraph({ name: "qa" })
  .agentNode("assistant", {
    llm: mockLLM(),
    prompt: { system: "You are a concise assistant. Prefix your final answer with FINAL:." },
    maxIterations: 2
  })
  .compile();

const result = await app.run({});
console.log(result.status);                          // "completed"
console.log(result.channels.agentResult.reasoning);  // the ReAct reasoning trace
```

## The `agentNode` config

| Field | Meaning |
| --- | --- |
| `llm` | The `LLMGateway` the agent runs on. |
| `prompt` | `{ system: "..." }` inline, or `{ registry, id, version? }` from a `PromptRegistry`. |
| `tools` | A `ToolRegistry` the agent may call (see [tools](./tools-and-tool-nodes)). |
| `tier` | A capability tier: `"frontier" \| "balanced" \| "fast" \| "creative"`. |
| `model` / `provider` | Pin a concrete model/provider (an explicit `model` always wins over `tier`). |
| `maxIterations` | Cap on the ReAct reasoning loop. |
| `suspendForApproval` | Suspend the run when a gated tool is reached (see [approval gates](/docs/governance/approval-gates)). |
| `outputChannel` | Channel the result lands in (default `"agentResult"`). |

The result type is `AgentResult` (from `@ailu-ai/agents-core`):
`{ artifacts, blockers, approvalRequests, confidence, reasoning, requiresHumanReview }`. The
output channel holds the **full object** — you'll most often route on `confidence` or
`requiresHumanReview`.

## Capability tiers, not hardcoded models

Declare a **tier** and let the engine resolve it against the available providers:

```ts
createGraph({ name: "tiered" })
  .agentNode("writer", {
    llm: mockLLM(),
    prompt: { system: "Draft a short release note." },
    tier: "balanced"   // resolved to a concrete model by ModelPolicy (env-aware)
  })
  .compile();
```

- On the **Rust** path the bridge resolves the tier from the process env (e.g. with only
  `MISTRAL_API_KEY` set, every tier maps to the Mistral column).
- On the **TS** fallback path the SDK resolves it against `availableFromEnv()`.
- An explicit `model` (and `provider`) always overrides the tier.

## Routing on the agent's result

Because the result lands in a typed channel, route on it with a conditional edge — for example,
send a flagged answer to a human gate:

```ts
createGraph({ name: "reviewed-qa" })
  .agentNode("assistant", { llm: mockLLM(), prompt: { system: "Answer." } })
  .humanGate("review")
  .node("publish", async () => ({ published: true }))
  .conditionalEdge("assistant", "review", "needsReview", (s) => s.channels.agentResult.requiresHumanReview)
  .conditionalEdge("assistant", "publish", "isClean", (s) => !s.channels.agentResult.requiresHumanReview)
  .edge("review", "publish")
  .compile();
```

An agent **never approves its own output** — review is always a different principal. The full
governance loop is in [approval gates](/docs/governance/approval-gates).

## Prebuilt micro-agents

For common single-purpose agents, `prebuilt` gives you a ready-to-run `CompiledGraph`. Each runs
on a deterministic mock gateway by default (no keys).

```ts
import { prebuilt } from "@ailu-ai/graph-sdk";

const result = await prebuilt.summarizer().run({ question: "…a long text…" });
const summary = result.channels.summary; // AgentResult
```

The 16 prebuilt agents and the channel each writes:

| Agent | Tier | Output channel |
| --- | --- | --- |
| `summarizer` | fast | `summary` |
| `classifier` | fast | `label` |
| `extractor` | fast | `extracted` |
| `translator` | fast | `translation` |
| `sentimentAnalyzer` | fast | `sentiment` |
| `entityExtractor` | fast | `entities` |
| `piiRedactor` | fast | `redacted` |
| `intentClassifier` | fast | `intent` |
| `titleGenerator` | fast | `title` |
| `keywordExtractor` | fast | `keywords` |
| `sqlGenerator` | balanced | `sql` |
| `questionAnswerer` | balanced | `answer` |
| `ragAnswerer` | balanced | `answer` |
| `refundApprover` | balanced | `refundDecision` |
| `codeReviewer` | frontier | `review` |
| `copyEditor` | creative | `edited` |

Override the gateway, model or tier per call:

```ts
prebuilt.classifier({ tierOverride: "balanced" });
prebuilt.summarizer({ model: "claude-opus-4-8" });
prebuilt.questionAnswerer({ llm: myGateway });
```

`ragAnswerer` is a composed graph (retriever + reranker + an agent step) and accepts `docs`,
`k`, and `questionChannel`.

## Next

[Tools and tool nodes →](./tools-and-tool-nodes)
