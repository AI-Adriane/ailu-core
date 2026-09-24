---
sidebar_position: 12
title: Choosing a model (the `model` surface)
description: One import, one mental model — model.openai("gpt-4o").invoke(), zero-config model.invoke(), tiers as properties. Executed in the one Rust engine.
tags: ["models"]
difficulty: beginner
---

# Choosing a model

One import, one surface (ADR 0034). The call executes in the **one Rust engine** — no TS provider
client, one consistent behaviour:

```ts
import { model, createGraph } from "@ailu-ai/graph-sdk";

// zero-config: provider resolved from whichever API key is in your env; fails loud if none
await model.invoke("Summarize this PR in one line.");

// the one-liner: the provider IS the method
await model.openai("gpt-4o").invoke("hi");
await model.anthropic("claude-opus-4-8").invoke("hi");

// tiers are properties — let the engine pick the model (and provider, when omitted)
await model.fast.invoke("classify: spam?");
await model.anthropic.frontier.invoke("hi");

// drop the same value into a graph — no re-wrap
createGraph({ name: "assistant" })
  .agentNode("triage", { model: model.openai.fast, prompt: { system: "Classify." } })
  .agentNode("reply",  { model: model.anthropic("claude-opus-4-8"), prompt: { system: "Answer." } })
  .compile();
```

`model.cohere` is a **compile error** (not a key on the surface); an unknown provider at runtime
throws `UnknownProviderError` — never a silent default.

## Typed structured output

`.output(schema)` constrains the response to a JSON Schema (driven natively per provider in the
engine, ADR 0029) and types the result — still a single engine call. The schema is any
`{ jsonSchema, parse }` (no hard Zod dependency); wrap a Zod schema in one line:

```ts
import { z } from "zod";

const Triage = z.object({ severity: z.enum(["low", "high"]), area: z.string() });

const out = await model
  .openai("gpt-4o")
  .output({
    jsonSchema: {
      type: "object",
      properties: { severity: { enum: ["low", "high"] }, area: { type: "string" } },
      required: ["severity", "area"]
    },
    parse: (v) => Triage.parse(v)
  })
  .invoke("db is down");

out.parsed.severity; // typed "low" | "high"
```

> A one-line `jsonSchema(zodSchema)` adapter (Zod → `{ jsonSchema, parse }`) is a small follow-up;
> the generic contract above keeps the SDK Zod-free today.

## API keys (from the environment)

Keys are read from the environment, fail-loud, never inlined:

- `model.openai(...)` reads `OPENAI_API_KEY` (per-provider defaults: `ANTHROPIC_API_KEY`,
  `GEMINI_API_KEY`, `MISTRAL_API_KEY`, …); override with `model.openai("gpt-4o", { apiKeyEnv: "CORP_KEY" })`.
- A named provider with no key → `MissingProviderKeyError` (names the exact var to set).
- A provider-less `model.fast` / `model.invoke()` with no key in env → `NoProviderInEnvError`.

## Custom / self-hosted endpoints

```ts
await model.ollama("llama3.3").invoke("hi");                  // local, keyless
await model.openaiCompatible({ baseURL: "http://localhost:1234/v1", model: "qwen2.5", apiKeyEnv: "VLLM_KEY" }).invoke("hi");
```

## Notes

- Every form resolves to a plain `ModelSpec` (`{ provider?, model?, tier?, baseURL?, apiKeyEnv? }`)
  — methodless data that crosses the napi/pyo3 wire. Zero runtime cost; `.invoke()` is a single
  engine call. No TS HTTP client.
- The per-provider packages (`@ailu-ai/model-openai`, `-anthropic`, …) still exist (ADR 0031);
  `model.<provider>` is the unified front door over them.
- `@ailu-ai/graph-sdk` installs **without** any provider SDK (ADR 0034 16a).
- See [ADR 0034](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0034-model-surface-and-env-key-resolution.md)
  and [ADR 0031](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0031-per-model-provider-packages.md).
