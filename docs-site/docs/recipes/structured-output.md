---
sidebar_position: 8
title: Structured output (typed JSON from an agent)
description: Constrain an agent to a JSON Schema, validated in-engine, with a bounded retry and a fail-closed/lenient mode.
tags: ["models"]
difficulty: intermediate
---

# Structured output

Make an agent return **JSON that matches a schema** instead of free text — generated natively per
provider (OpenAI `response_format`, Anthropic forced tool, Gemini `responseSchema`) **and validated
in the Rust engine** before it reaches you (ADR 0029). Add it as one efficiency-middleware entry:

```ts
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "triage" })
  .agentNode("classify", {
    model: openai("gpt-4o"),
    prompt: { system: "Classify the support ticket." },
    middleware: [
      {
        kind: "structuredOutput",
        params: {
          name: "Triage",
          schema: {
            type: "object",
            properties: {
              severity: { type: "string", enum: ["low", "high"] },
              summary: { type: "string" }
            },
            required: ["severity", "summary"]
          }
        }
      }
    ]
  })
  .compile();

const result = await app.run({ ticket: "the checkout page 500s on submit" });
const triage = result.channels.agentResult.structuredOutput; // { severity, summary } — already validated
```

## Modes

| `params` | Effect |
| --- | --- |
| `schema` (required) | The JSON Schema the output must match. Nested / enum / format are all enforced (real conformance). |
| `mode: "required"` (default) | Invalid output → a **bounded deterministic retry** (`retryCap`, default 2), then **fail closed** (a typed error surfaced as channel data — never a crash). |
| `mode: "lenient"` | Invalid output falls back to raw text (advisory use). |
| `name`, `strict`, `retryCap` | Schema name, provider strict-decoding flag, retry budget. |

## Why it's different

Validation runs **in the engine**, so a worker-executed run can never emit unvalidated output, and
the verdict is part of the auditable run state. The approval gate stays intrinsic — a structured
result can't route around a sensitive-tool gate. See [ADR 0029](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0029-governed-structured-output.md).

## Next
- [Middleware & profiles](/docs/advanced-agents/middleware-and-profiles)
