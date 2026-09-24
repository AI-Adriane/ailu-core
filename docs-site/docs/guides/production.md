---
title: "Deploy to production"
description: "Where Ailu runs, how to keep runs across restarts and instances, and the settings to check before going live."
---

# Deploy to production

Ailu is a library: it runs inside your own Node.js service. There is no Ailu server to deploy.

## The usual shape

1. **Your API** starts runs with the catalog runner and stores the returned state in your
   database, keyed by `runId`.
2. **Your UI** shows suspended runs to the people who approve them.
3. **On approval** (or on a webhook, or at a date), your API or a queue worker loads the state
   and calls `resumeCatalogGraph`.

Because the state is plain JSON in your database, any instance can resume any run, and a deploy
or a crash loses nothing that was checkpointed. See [Long-running runs](./long-running.md).

`app.run()` / `app.resume()` keep suspended runs in the memory of one `CompiledGraph`. They suit
runs that finish within one request or one worker, and single-instance services.

## Platform

- Node.js 22 or later.
- Linux with glibc (x64 or arm64), macOS or Windows x64. In Docker, use a Debian- or Ubuntu-based
  image such as `node:22-slim`, not Alpine.
- Call `rustEngineAvailable()` at startup and fail fast if it returns `false`.

## Checklist

| Check | Why |
| --- | --- |
| API keys come from your secret manager, as environment variables | Keys never appear in code or in graph definitions. |
| `AILU_LLM_MOCK` is **not** set | It answers with a mock instead of failing when a key is missing. |
| `resolvedBy` comes from your authenticated session | It is the approver of record. See [Tools and approval](./tools.md). |
| `AILU_PII_REDACTOR_URL` and `AILU_PII_REDACTOR_FAIL_CLOSED=1`, if you handle personal data | Personal data is redacted before it reaches a model, and nothing is sent if the redactor is down. See [Governance](./governance.md). |
| `AILU_SECRETS_POLICY=block` for strict environments | A prompt containing a secret fails instead of being masked. |
| An `ApprovalEngine` backed by your database | Approval requests and decisions survive restarts. |
| `AILU_LLM_RECORD=1` for runs you may have to justify | You can replay them later. |
| Tracing: `exportTracesToOtlp(app)` for `app.run()`; the `onEvent` option of the catalog runner otherwise | Traces and cost for every run. See [Observability](./observability.md). |
| `AILU_HTTP_READ_TIMEOUT_SECS` | Timeout for model calls (default 600 s). |

Every variable is listed in [Environment variables](../reference/environment.md).

## Test before you ship

Run your graphs in CI with `AILU_LLM_MOCK=1`: agents answer from the deterministic mock (each tool
is called once, then the agent answers `done`), so tests are fast, free and repeatable. Test the
wiring: the gates suspend, approvals resume, the right channels get written.
