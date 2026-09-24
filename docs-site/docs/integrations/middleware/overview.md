---
sidebar_position: 1
title: Middleware overview
description: The agent middleware catalog — a sealed governed layer (redaction, the intrinsic approval gate, fs policy) and a user-tunable efficiency layer (compress, terse, contextBudget, reflection, structuredOutput).
---

# Middleware overview

Every agent node runs its ReAct loop through one ordered **middleware stack** (ADR 0025). The
stack is two layers, and the split is the whole point: an **ungoverned agent is
unrepresentable**.

- **GOVERNED** — PII redaction, the human-approval gate, the filesystem policy. Engine-injected
  and **sealed**: you cannot add to it, remove from it, or turn it off.
- **EFFICIENCY** — compression, terse output, context-budget trim, reflection, structured output.
  **User-tunable**: the layer you compose with `profile` and `middleware`.

A user can only ever append to the efficiency layer. The governed layer is always present, and
the approval gate is **intrinsic** to `before_tool` — it fires even on an empty stack, so a bare
agent still gates its sensitive tools.

This page is the catalog. For the full guide — profiles, desugaring, lifecycle hooks, and
examples — see [Middleware & profiles](/docs/advanced-agents/middleware-and-profiles).

## Usage

You compose efficiency middleware through the SDK; the governed layer is injected for you.

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

createGraph({ name: "deep" })
  .agentNode("worker", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Investigate and report." },
    // EFFICIENCY layer — user-tunable. The GOVERNED layer is engine-injected and sealed.
    middleware: [
      { kind: "compress" },
      { kind: "terse" },
      { kind: "contextBudget", params: { chars: 8000 } },
      { kind: "reflection" }
    ]
  })
  .compile();
```

## The catalog

`kind` is the spec passed to `middleware` (efficiency only); `hook` is the ReAct-loop lifecycle
point it runs at.

| Kind | Hook | Effect | Layer |
| --- | --- | --- | --- |
| `redact` | `before_model` (scrub, fail-closed) / `after_model` (hydrate) | PII redaction (ADR 0008). A redaction-block `Err` short-circuits the run. | GOVERNED |
| *(approval gate)* | `before_tool` | A `requires_approval` tool is gated for a human unless this exact grant was already approved — no self-approval. Intrinsic; fires even on an empty stack. | GOVERNED |
| *(fs policy)* | `before_tool` | Allow / deny / gate guarded filesystem writes (ADR 0024). | GOVERNED |
| `compress` | `before_model` | Route `user`-role content through the prompt-compression service (ADR 0014). **Fail-open** — a no-op when the service is not configured. | EFFICIENCY |
| `terse` | `before_model` | Append a compact-output directive to the system prompt (ADR 0014). Lossy — prose only, not code. | EFFICIENCY |
| `contextBudget` | `before_run` | Cap the agent's seed message (the injected `Input` / `State` dump) to `params.chars` characters; truncates on a char boundary, marks the cut with `…`. | EFFICIENCY |
| `reflection` | `after_run` | One self-critique over the run's reasoning (ADR 0025). On rejection, annotates with a `reflection:needs_review:<issues>` marker. **Fail-open**; never forces a suspend. | EFFICIENCY |
| `structuredOutput` | `before_model` (sets the provider's native constraint) / `after_model` (validates) | Constrain the agent's output to a JSON Schema (ADR 0029). Provider-neutral: OpenAI `response_format`, Anthropic forced tool, Gemini `responseSchema`, plus an in-engine JSON-Schema validation floor. The validated value lands on `AgentResult.structuredOutput`. `params.mode: "required"` (default) fails closed after `params.retryCap` (default 2) deterministic re-prompts; `"lenient"` falls back to raw text. | EFFICIENCY |

The governed `redact` / approval-gate / fs-policy rows have **no `kind` you can pass** — they are
engine-injected. Only `compress`, `terse`, `contextBudget`, `reflection`, and `structuredOutput`
are valid `middleware` specs.

## Governed kinds are rejected from user middleware

The governance middleware are *not* part of the `middleware` type — they are not
`EfficiencyMiddlewareSpec`s. Passing one (e.g. from untyped JavaScript) throws:

```ts
import { GovernanceMiddlewareRejectedError } from "@ailu-ai/graph-sdk";

// { kind: "redact" } is not an EfficiencyMiddlewareSpec — the SDK rejects it.
```

This is *governed-by-construction*: you cannot express "an agent without the approval gate", and
you cannot smuggle a governance middleware into the user layer.

## Ordering

Request-path hooks run **governed → efficiency** (governance outermost); response-path hooks run
in reverse (onion semantics). Within the efficiency layer order does not change behaviour — the
hooks act on disjoint parts of the request.

## Configuration (governed seams)

The governed layer reaches external services through env vars — exact names:

| Env var | Used by | Notes |
| --- | --- | --- |
| `AILU_PII_REDACTOR_URL` | `redact` | External redaction service (required to enable HTTP redaction). |
| `AILU_PII_REDACTOR_TOKEN` | `redact` | Optional bearer token for the redactor. |
| `AILU_LLMLINGUA_URL` | `compress` | External prompt-compression service (`POST { text, rate } -> { compressed }`). Unset → `compress` is a no-op. |
| `AILU_LLMLINGUA_RATE` | `compress` | Target keep-ratio (default `0.5`). |
| `AILU_LLMLINGUA_MIN_CHARS` | `compress` | Minimum message length before compression applies. |

Both the redactor and the compressor are **external seams** (HTTP services you run); the engine
ships the composition, not the model behind it.

## Next

- [Middleware & profiles →](/docs/advanced-agents/middleware-and-profiles) — the full guide:
  profiles, desugaring rules, lifecycle hooks, and reflection routing.
