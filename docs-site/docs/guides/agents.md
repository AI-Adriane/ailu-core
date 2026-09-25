---
title: "Agents and models"
description: "Add an LLM agent to a graph, choose its model, read its answer, and get structured output."
---

# Agents and models

An agent node runs an LLM in a loop: it reads the run's state, may call tools, and ends with an
answer.

```ts file=packages/graph-sdk/examples/docs/agents-basic.ts region=example
```

## What the agent sees

The agent gets its system prompt and the run's state: every channel, as JSON. Show it less with
`visibleChannels`, which keeps the prompt small and keeps unrelated data away from the model:

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=visible-channels
```

## Read the answer

The agent writes its result to the `agentResult` channel. Use `outputChannel` to pick another
name, which you need when a graph has several agents.

| Field | What it holds |
| --- | --- |
| `reasoning` | The agent's steps; the answer follows the last `final:` marker. Read it with `finalAnswer(result)`. |
| `structuredOutput` | The parsed JSON when you asked for [structured output](#get-structured-output). |
| `usage` | Tokens used across the agent's model calls. |
| `approvalRequests` | Tool calls waiting for a human. See [Tools and approval](./tools.md). |
| `todos` | The agent's plan, when it uses the todo tool. See [Deep agents](./deep-agents.md). |

## Choose a model

Pass a model to `model:`. Everything below is exported as `model` from `@ailu-ai/graph-sdk`:

```ts file=packages/graph-sdk/examples/docs/agents-models.ts region=example
```

With a tier (`model.fast`, `model.balanced`, `model.frontier`), the engine picks the provider
from the keys you have set, in this order: Anthropic, OpenAI, Gemini, Mistral, OpenRouter,
MiniMax, Hugging Face. The model each tier maps to is listed in
[Models and providers](../reference/models.md).

Each provider reads its key from one environment variable (see [Install](../install.md#set-an-api-key)).
If the key is missing, the run fails with an error that names it. Set `AILU_LLM_MOCK=1` to run
offline on the deterministic mock.

## Get structured output

Give the agent a JSON Schema. The provider is asked for JSON that matches it, and the parsed value
lands in `structuredOutput`:

```ts file=packages/graph-sdk/examples/docs/agents-structured-output.ts region=example
```

With `mode: "required"` (the default), the agent retries when the answer doesn't match, then
reports an error in its result. With `"lenient"`, it keeps the plain answer and leaves
`structuredOutput` empty.

## Control cost and length

| Option | Effect |
| --- | --- |
| `maxIterations` | Caps the agent's loop (model call → tool calls → model call …). |
| `middleware: [{ kind: "terse" }]` | Asks for short answers. Saves output tokens on prose; don't use it for code. |
| `middleware: [{ kind: "contextBudget", params: { chars: 8000 } }]` | Trims the state the agent is shown. |
| `middleware: [{ kind: "compress" }]` | Compresses long prompts through an [LLMLingua](https://github.com/microsoft/LLMLingua) service at `AILU_LLMLINGUA_URL`. Does nothing when it is not set. |

## Call a model directly

For a single completion you don't need a graph:

```ts file=packages/graph-sdk/examples/docs/agents-invoke.ts region=example
```

And for typed JSON, `.output()`:

```ts file=packages/graph-sdk/examples/docs/agents-invoke.ts region=typed
```

## Next

- Let the agent act: [Tools and approval](./tools.md).
- Watch tokens as they arrive: [Streaming](./streaming.md).
- Several agents working together: [Multi-agent](./multi-agent.md).
