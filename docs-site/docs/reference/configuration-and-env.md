---
sidebar_position: 6
title: Configuration and environment
description: Every environment variable Ailu reads — engine seam URLs and provider keys on the Rust path, the control-plane FEATURE_ flags, and the validated control-plane env.
---

# Configuration and environment

Ailu is configured by environment variables in three groups. The **engine** reads its
variables directly on the Rust path (provider credentials, the local-server flags, and the
optional seam URLs that wire redaction, secrets, compression, and the durable filesystem to
external services). The **control plane** validates its own set through a typed parser and reads
the `FEATURE_` flags. A few variables are read by TypeScript-side conveniences (the OTLP exporter,
the MCP server) and are called out as such.

Nothing here is set in code: secrets live only in env, `.env` is gitignored, and the engine never
hardcodes a key. An engine seam stays off until its URL is set, so the default posture is
"in-process only, no outbound calls".

## Engine variables (Rust path)

These are read inside the Rust engine when it builds a run. Provider keys gate which LLM providers
are usable; the seam URLs each turn on an external service for one governed concern.

### Provider credentials

A provider is usable iff its credential is present in the process environment. The keyless local
servers are flag-gated instead.

| Variable | Provider | Notes |
| --- | --- | --- |
| `ANTHROPIC_API_KEY` | Anthropic | Optional; non-empty when set. |
| `OPENAI_API_KEY` | OpenAI | Optional; non-empty when set. |
| `GEMINI_API_KEY` | Google | Falls back to `GOOGLE_API_KEY` when unset. |
| `GOOGLE_API_KEY` | Google | Alternative to `GEMINI_API_KEY`. |
| `MISTRAL_API_KEY` | Mistral | Optional; non-empty when set. |
| `OPENROUTER_API_KEY` | OpenRouter | Optional; non-empty when set. |
| `MINIMAX_API_KEY` | MiniMax | Optional; non-empty when set. |
| `HF_TOKEN` | Hugging Face | Optional; non-empty when set. |
| `AILU_USE_OLLAMA` | Ollama (local) | Keyless. Enabled only when set to exactly `1`. |
| `AILU_USE_LMSTUDIO` | LM Studio (local) | Keyless. Enabled only when set to exactly `1`. |
| `AILU_OLLAMA_BASE_URL` | Ollama (local) | Overrides the host. Unset → `http://localhost:11434/v1`. |
| `AILU_LMSTUDIO_BASE_URL` | LM Studio (local) | Overrides the host. Unset → `http://localhost:1234/v1`. |

With no credential and no local flag, the engine resolves to the deterministic `mock-model` — runs
still execute, they just don't call a real provider. The model policy walks providers in preference
order and picks the highest-preference one that is both available and in the tier table.

### Governed seam URLs

Each seam is **off when its URL is unset or empty** and wraps the governed layer when set. They sit
inside the sealed governance stack, so they see text before any provider does.

| Variable | Default | Seam |
| --- | --- | --- |
| `AILU_PII_REDACTOR_URL` | unset → off | External PII redaction service. Wire contract: `POST { "texts": [...] } -> { "texts": [...] }`, same length and order. **Fail-open** (a transport error passes text through unchanged). |
| `AILU_PII_REDACTOR_TOKEN` | unset | Optional bearer token for the PII redactor. |
| `AILU_SECRETS_POLICY` | `mask` | Policy for the always-on in-engine regex secrets floor. `block` fails the call closed after scrubbing; anything else masks. The floor runs even with no URL set. |
| `AILU_SECRETS_REDACTOR_URL` | unset → off | Optional external secrets augmentation (defense-in-depth), reusing the PII redactor wire shape. |
| `AILU_SECRETS_REDACTOR_TOKEN` | unset | Optional bearer token for the external secrets redactor. |
| `AILU_FS_BACKEND_URL` | unset → in-memory | External durable filesystem backend. **Fail-closed** (a transport error is a service-unavailable error, never a silent pass). When unset, the governed fs is in-memory and does not survive suspend/resume across the native boundary. |
| `AILU_FS_BACKEND_TOKEN` | unset | Optional bearer token for the fs backend. |
| `AILU_LLMLINGUA_URL` | unset → off | External LLMLingua prompt-compression service. Wire contract: `POST { text, rate } -> { compressed }`. |
| `AILU_LLMLINGUA_RATE` | `0.5` | Target keep-ratio for compression. |
| `AILU_LLMLINGUA_MIN_CHARS` | `240` | Skip texts shorter than this many characters. |

The in-engine secrets floor (governed by `AILU_SECRETS_POLICY`) is **always on** and is pushed
first, so keys and tokens are scrubbed before any text reaches the external PII service — even when
`AILU_PII_REDACTOR_URL` is unset.

### Engine selection

| Variable | Default | Meaning |
| --- | --- | --- |
| `AILU_SDK_ENGINE` | `auto` | `auto` or `rust` both select the Rust engine. `ts` is **no longer supported** — the TypeScript engine fallback was removed, so `AILU_SDK_ENGINE=ts` raises `RustEngineRequiredError` at compile time. |

There is no TypeScript fallback: Ailu runs exclusively on the Rust engine via `@ailu-ai/napi`.
`AILU_SDK_ENGINE` is an escape hatch that survives only to force-select Rust and to reject `ts`;
it cannot switch you onto a TS runtime because none exists.

## Control-plane FEATURE_ flags

Feature flags are read by the control plane, not the engine. The flag set is closed: five flags,
all defaulting to **disabled**. A flag named `multi-agent` maps to the env key `FEATURE_MULTI_AGENT`
(dashes → underscores, uppercased, `FEATURE_` prefix). A value is truthy only when its trimmed,
lowercased form equals exactly `"true"` — `1`, `yes`, and `on` are all read as off.

| Env key | Flag | Default |
| --- | --- | --- |
| `FEATURE_STREAMING` | `streaming` | off |
| `FEATURE_SUBGRAPHS` | `subgraphs` | off |
| `FEATURE_MULTI_AGENT` | `multi-agent` | off |
| `FEATURE_EVAL` | `eval` | off |
| `FEATURE_FLEET` | `fleet` | off |

## Control-plane variables

The control plane validates its environment through a typed parser that collects **all** failures
into one aggregate error (a missing required key, an out-of-range port, and an invalid enum surface
together, not one at a time). Required keys with no default abort startup.

| Variable | Required | Default | Notes |
| --- | --- | --- | --- |
| `NODE_ENV` | yes | — | One of `local`, `staging`, `production`. |
| `DATABASE_URL` | yes | — | Non-empty. |
| `REDIS_URL` | yes | — | Non-empty. |
| `JWT_SECRET` | yes | — | Non-empty. |
| `PORT` | no | `3000` | Positive integer within the valid port range. |
| `JWT_EXPIRY` | no | `1h` | Non-empty when set. |
| `LOG_LEVEL` | no | `info` | One of `debug`, `info`, `warn`, `error`. |
| `OPENAI_API_KEY` | no | unset | Non-empty when present. |
| `ANTHROPIC_API_KEY` | no | unset | Non-empty when present. |
| `MISTRAL_API_KEY` | no | unset | Non-empty when present. |
| `OTEL_ENDPOINT` | no | unset | Non-empty when present. |

The optional keys are validated for non-emptiness when present: setting `OPENAI_API_KEY=` (empty)
is an error, while omitting it entirely is fine.

## TypeScript-side conveniences

A couple of variables are read by TypeScript helpers around the engine, not by the Rust engine path
itself. They configure optional, fail-open tooling.

| Variable | Default | Read by | Meaning |
| --- | --- | --- | --- |
| `AILU_OTEL_EXPORTER_URL` | unset → no-op | OTLP exporter in `@ailu-ai/graph-sdk` | OTLP/HTTP traces endpoint for `exportTracesToOtlp`. When unset, the exporter is a no-op. **Fail-open** (an export error never fails a run). |
| `AILU_MCP_KB_NAMESPACE` | `ailu` | the Ailu MCP server | Knowledge-base namespace the MCP server reads from. |

These run in TypeScript: the Rust engine emits the lifecycle events the OTLP exporter subscribes to,
but the export itself, and the MCP server, are TS processes outside the engine.

## See also

- [Events and streams](./events-and-streams) — the run-lifecycle events the OTLP exporter consumes.
- [Errors](./errors) — `RustEngineRequiredError` and the rest of the typed error set.
- [Governed filesystem](/docs/advanced-agents/governed-filesystem) — what `AILU_FS_BACKEND_URL` makes durable.
- [Built for AI agents](./built-for-ai-agents) — the agent-legible surface around this configuration.