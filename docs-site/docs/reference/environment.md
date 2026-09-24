---
title: "Environment variables"
description: "Every environment variable the SDK and the engine read."
---

# Environment variables

The engine reads these at the moment it needs them, so a change applies to the next run.

## Model providers

| Variable | Used for |
| --- | --- |
| `ANTHROPIC_API_KEY` | `model.anthropic(...)` |
| `OPENAI_API_KEY` | `model.openai(...)`, and `createEmbeddings({ provider: "openai" })` |
| `GEMINI_API_KEY` or `GOOGLE_API_KEY` | `model.gemini(...)` |
| `MISTRAL_API_KEY` | `model.mistral(...)`, and `createEmbeddings()` (Mistral is the default provider) |
| `OPENROUTER_API_KEY` | `model.openrouter(...)` |
| `MINIMAX_API_KEY` | `model.minimax(...)` |
| `HF_TOKEN` or `HUGGINGFACE_API_KEY` | `model.huggingface(...)` |
| `AILU_USE_OLLAMA=1`, `AILU_OLLAMA_BASE_URL` | Turn on `model.ollama(...)`; the server URL (default `http://localhost:11434/v1`). |
| `AILU_USE_LMSTUDIO=1`, `AILU_LMSTUDIO_BASE_URL` | Turn on `model.lmstudio(...)`; the server URL (default `http://localhost:1234/v1`). |
| The variable you name in `apiKeyEnv` | The key for a `model.openaiCompatible(...)` endpoint. |
| `AILU_LLM_MOCK=1` | Offline mode: calls without a key answer from the deterministic mock instead of failing. For tests and CI. |
| `AILU_HTTP_READ_TIMEOUT_SECS` | How long to wait for a model response. Default 600. |

## Governance

| Variable | Used for |
| --- | --- |
| `AILU_SECRETS_POLICY=block` | Fail a model call that contains a secret, instead of masking the secret. |
| `AILU_SECRETS_REDACTOR_URL`, `AILU_SECRETS_REDACTOR_TOKEN` | An extra secrets-detection service, after the built-in one. |
| `AILU_PII_REDACTOR_URL`, `AILU_PII_REDACTOR_TOKEN` | A personal-data redaction service. |
| `AILU_PII_REDACTOR_FAIL_CLOSED=1` | Don't send the text when the redaction service is unreachable. |
| `AILU_LLM_RECORD=1` | Record model calls and timestamps, for [replay](../guides/governance.md#replay-a-run). |

## Agents and retrieval

| Variable | Used for |
| --- | --- |
| `AILU_FS_BACKEND_URL`, `AILU_FS_BACKEND_TOKEN` | A storage service for the agents' virtual filesystem, instead of memory. |
| `AILU_RERANK_ENDPOINT` | A cross-encoder service for `components.reranker`. |
| `AILU_LLMLINGUA_URL`, `AILU_LLMLINGUA_RATE`, `AILU_LLMLINGUA_MIN_CHARS` | A prompt-compression service for the `compress` middleware; the rate (default 0.5) and the shortest prompt it compresses. |
| `TAVILY_API_KEY` | `components.webSearch`. |

## Observability

| Variable | Used for |
| --- | --- |
| `AILU_OTEL_EXPORTER_URL` | The default endpoint of `exportTracesToOtlp`. |
