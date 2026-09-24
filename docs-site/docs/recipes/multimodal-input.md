---
sidebar_position: 9
title: Multimodal input (send an image to an agent)
description: Attach image/audio/file content blocks to an agent run via a channel, fanned out per provider.
tags: ["models"]
difficulty: beginner
---

# Multimodal input

Send an agent **image / audio / file** content alongside text (ADR 0030). The engine carries a
provider-neutral content block and fans it out to each provider's wire form (OpenAI `image_url`,
Anthropic `image` blocks, Gemini `inlineData`). Text-only runs are byte-identical to before.

Bind a channel that carries the media, and seed it on the run:

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "vision" })
  .channel("__media", { type: "json", default: [] as unknown[] })
  .agentNode("describe", {
    model: openai("gpt-4o"),
    prompt: { system: "Describe the image." },
    inputBlocksChannel: "__media" // this channel's ContentBlock[] becomes the multimodal seed
  })
  .compile();

await app.run({
  __media: [
    { type: "text", source: undefined, text: "What is in this picture?" },
    { type: "image", source: { kind: "base64", mediaType: "image/png", data: "<base64>" } }
  ]
});
```

## Media sources

| `source.kind` | Use |
| --- | --- |
| `base64` | Inline bytes `{ mediaType, data }` — size-capped. |
| `url` | A stable / content-addressed URL `{ url, mediaType? }`. |
| `artifact` | A small `{ artifactId, version?, mediaType }` pointer resolved to bytes at the gateway boundary — keeps checkpoints small (the **default**, ADR 0030). |

The bound channel is **excluded from the stringified State**, so binary bytes are never re-fed as
text. Output media a chat model returns (e.g. Gemini inline images) surfaces on
`LlmResponse.content_blocks`. See [ADR 0030](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0030-multimodal-input.md).
