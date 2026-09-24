---
sidebar_position: 5
title: PII redaction seam
description: Scrub personal data from every LLM request before it reaches a provider — a no-op by default, wired to your own redaction service via one env var.
---

# PII redaction seam

The engine routes **every** LLM request — the system prompt and every intermediate message
(tool observations, prior turns fed back into the agent loop) — through an optional
**`PiiRedactor`** seam before it reaches a provider. By default the seam is a **no-op**: the
open-source engine ships the hook, not the detector. Point one env var at a redaction service
and every outbound call is scrubbed.

This closes the gap that redacting only at your application boundary leaves open: an agent that
reads a customer record mid-run would otherwise send that PII to the model in its next turn.

## Enable it

```bash
# The engine POSTs outbound texts here and uses the redacted reply.
export AILU_PII_REDACTOR_URL="https://your-redactor.internal/redact-batch"
# Optional shared secret, sent as a bearer token.
export AILU_PII_REDACTOR_TOKEN="…"
```

Unset → the engine runs with no redaction and no extra network hop (the no-op default). The
seam is compiled into the native addon, so it is live as soon as the URL is set — no rebuild.

## The wire contract

Deliberately tiny. The engine sends a batch; your service returns the same texts, same length,
same order, redacted:

```http
POST /redact-batch
Authorization: Bearer <AILU_PII_REDACTOR_TOKEN>   # only if configured
Content-Type: application/json

{ "texts": ["Email me at alice@example.com", "…"] }
```

```json
{ "texts": ["Email me at [EMAIL]", "…"], "blocked": false }
```

- **`texts`** — the redacted strings, written back into the request in order.
- **`blocked`** — set `true` to **stop** the call (see below).

Anything that speaks this contract works: a thin wrapper over
[Microsoft Presidio](https://microsoft.github.io/presidio/) or
[GLiNER-PII](https://github.com/urchade/GLiNER), an OpenAI privacy pass, or your own regex
service.

## Behavior

| Service replies | Engine does |
| --- | --- |
| `{ texts, blocked: false }` | Writes the redacted texts back, then calls the provider. |
| `{ texts, blocked: true }` | **Fails the call** (`LlmError::PiiBlocked`). The agent node surfaces an error result instead of an answer — a blocked agent does not crash the graph, and the personal data never reaches a provider. |
| HTTP error / unreachable | **Fail-open**: the original text passes through, with a warning logged. |

Fail-open is deliberate: a flaky redaction service must not abort otherwise-valid runs. The
**hard** guarantee belongs at your input boundary (reject or redact untrusted input *before* the
run); this seam is defense-in-depth for everything the agent generates *during* a run.

## A minimal redaction service

A reference implementation in any language is a few lines. Node:

```js
import { createServer } from "node:http";

const EMAIL = /[\w.+-]+@[\w.-]+\.\w+/g;

createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const { texts } = JSON.parse(body);
    const redacted = texts.map((t) => t.replace(EMAIL, "[EMAIL]"));
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ texts: redacted, blocked: false }));
  });
}).listen(8088);
```

Swap the regex for a Presidio call to get entity detection, confidence thresholds, and a wider
set of entity types. Keep the placeholder↔value mapping (the "vault") **inside your service** if
you need to re-hydrate the model's final answer — the engine seam only redacts the outbound
side; it never sends the mapping over the wire.

## What the engine ships vs what you bring

| Engine (open source) | You / your control plane |
| --- | --- |
| `PiiRedactor` trait, `NoopPiiRedactor`, `RedactingGateway` wrapper, `HttpPiiRedactor` client | The detection model (Presidio / GLiNER / …) |
| The env wiring + wire contract | The policy (which entities, thresholds, per-tenant rules) |
| Fail-open / block behavior | The placeholder vault + final-answer re-hydration |

This keeps heavy, Python-flavored detection out of the engine while letting any deployment add
real redaction. The design rationale is **ADR 0008** in the repository.
