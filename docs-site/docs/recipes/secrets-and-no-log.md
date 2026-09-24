---
sidebar_position: 10
title: Secrets redaction & no-log channels
description: API keys/tokens are scrubbed from outbound requests automatically; mark a channel so its value never hits events/logs.
tags: ["governance", "ops"]
difficulty: intermediate
---

# Secrets redaction & no-log channels

Two governed, always-on protections (ADR 0032) close the last obvious leaks.

## Secrets are scrubbed automatically (no config)

A **deterministic in-engine secrets redactor** runs on every agent (governed, always-on, offline).
Any well-known credential in an outbound prompt or tool output — `sk-…`, `AKIA…`, `ghp_…`,
`xox…`, `AIza…`, `sk_live_…`, JWTs, PEM private keys, `Bearer …` — is replaced with a typed
placeholder (`[REDACTED:OPENAI_KEY]` …) **before it reaches the provider**. Nothing to wire.

Opt into hard-stop instead of masking:

```bash
AILU_SECRETS_POLICY=block   # a detected secret fails the call (surfaced as channel data)
# optional external augmentation for org-specific secrets:
AILU_SECRETS_REDACTOR_URL=https://my-redactor/scrub
```

## Keep a channel out of events/logs

Mark a channel `noLog` so its value is masked in run events / logs (a `[REDACTED_NO_LOG]`
sentinel) — but **still checkpointed in full**, so resume/time-travel are unaffected
(*durability ≠ observability*):

```ts
const app = createGraph({ name: "kyc" })
  .channel("ssn", { type: "string", default: "", noLog: true }) // never emitted to events/logs
  .channel("decision", { type: "string", default: "" })
  .agentNode("review", { model: openai("gpt-4o"), prompt: { system: "Review the application." } })
  .compile();
```

Both seams are **governed by construction** (sealed, unrepresentable to omit). Known gap: free-text
`error`/`reason` strings on failure events aren't scrubbed yet. See
[ADR 0032](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0032-secrets-redaction-and-no-log.md).
