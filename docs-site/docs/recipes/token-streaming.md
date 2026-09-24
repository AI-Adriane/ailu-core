---
sidebar_position: 13
title: Per-token streaming (live "typing" output)
description: Stream an agent's generation token by token in messages mode — observational, opt-in, byte-identical to a non-streaming run.
tags: ["streaming", "models"]
difficulty: intermediate
---

# Per-token streaming

Stream an agent's generation **token by token** as it runs — for a live "typing" UI — while the
run stays deterministic and resumable. Streaming is **observational only**: it never changes what
the run records (ADR 0033).

This is the token-level companion to [Stream to a governance dashboard](./stream-to-dashboard.md):
that recipe relays node/tool **lifecycle** events; this one streams the LLM's tokens.

## Stream in `messages` mode

`stream(data, "messages")` projects each LLM token delta onto a `message_delta` event. Choosing
`messages` mode opts the run into token streaming automatically; every other path (`run()`, or the
`values`/`updates`/`debug` streams) leaves the agent on the non-streaming `complete()` call,
byte-for-byte unchanged.

```ts
import { createGraph, openai } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "assistant" })
  .agentNode("assistant", {
    model: openai("gpt-4o"),
    prompt: { system: "You are a concise assistant." }
  })
  .compile();

for await (const event of app.stream({ question: "Explain checkpoints." }, "messages")) {
  if (event.type === "message_delta") {
    process.stdout.write(event.delta); // tokens arrive as the model generates them
  }
}
```

All deltas of one agent turn share a `messageId`, so a client concatenates them into a single
growing message.

## On the catalog run path (`runCatalogGraph`)

The same per-token stream is available on the catalog path a control plane runs (e.g. a governed
`/ask`). `runCatalogGraph` returns the final outcome, so token deltas arrive over its `onEvent`
callback rather than an async iterator: opt in with `streamTokens: true` and read the `token_delta`
run events.

```ts
import { runCatalogGraph } from "@ailu-ai/graph-sdk";

await runCatalogGraph(definition, {
  initialData: { question: "Explain checkpoints." },
  streamTokens: true,
  onEvent: (event) => {
    if (event.type === "token_delta") {
      process.stdout.write(event.delta); // same tokens, same side channel
    }
  }
});
```

`streamTokens` defaults to `false` — a catalog run that doesn't set it takes the non-streaming
`complete()` path, byte-for-byte unchanged. To bridge the callback into an async generator (SSE),
buffer the deltas and drain them while the run promise settles.

## Guarantees

- **Real provider SSE.** The Anthropic (`messages.stream`), OpenAI-compatible (`stream: true`) and
  Gemini (`streamGenerateContent`) adapters stream natively. The mock gateway streams too.
- **Byte-identical results.** The token stream is a *view* of the generation. The authoritative
  response the agent acts on is the fully-assembled one the provider returns — identical in every
  field to the non-streaming path. So `AgentResult`, the conversation history, and every
  **checkpoint** are exactly what they would be without streaming. A run interrupted mid-stream
  resumes cleanly: the checkpoint never recorded partial tokens.
- **Never durable.** Token deltas ride a side channel that **bypasses the event bus**, so they
  never enter a checkpoint or the audit journal (`durability ≠ observability`). The journal still
  records exactly the node/tool lifecycle events it always did.

## Nested sub-agents (`mapAgents`)

When a `mapAgents` fan-out spawns one sub-agent per item, their token streams interleave on the
wire. Each delta is tagged with a `spawnId` (the input index = the deterministic merge order) and a
`parentRunId`, so a UI can demultiplex the concurrent streams and group each spawn's tokens by
`messageId`.

## Notes

- Token streaming is **opt-in per run**. If no consumer streams in `messages` mode, agents take the
  unchanged `complete()` path — no per-token overhead, no behaviour change.
- The provider SSE transport is verified against a live key; the stream-reassembly logic
  (`*StreamAccumulator`) is unit-tested offline with fixtures.
- See [ADR 0033](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0033-token-streaming-and-subagent-tagging.md)
  for the design and invariants.
