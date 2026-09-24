# ADR 0005 — Multi-provider LLM gateway (OpenAI-compatible family + native Gemini)

- Status: Accepted
- Date: 2026-06-19
- Builds on: [ADR 0003](0003-ts-engine-deprecated-sdk-on-rust.md)

## Context

The Rust `crates/llm-gateway` is the only crate allowed to talk to LLM providers
(ADR 0003 made the Rust engine the execution path; the TypeScript
`@ailu-ai/llm-gateway` is a deprecated fallback). Today it ships two real adapters:

- `AnthropicAdapter` — a native adapter over the Anthropic Messages API.
- `OpenAiCompatibleAdapter` — speaks the OpenAI `/chat/completions` shape and serves
  both **Mistral** cloud and a local **Ollama** server.

We want first-class connectors for a broader set of models:
`openai`, `google` (Gemini), `minimax`, `mistral`, `openrouter`, `huggingface`, and
local models (`ollama`, `lmstudio`).

Two facts shape the decision:

1. **Most of these speak the OpenAI Chat Completions shape.** OpenAI, OpenRouter,
   MiniMax, Hugging Face's router, LM Studio, Ollama, and Mistral all expose a
   `/chat/completions` endpoint. One adapter already covers this wire shape — the only
   thing that varies is base URL, auth, and the default model.
2. **Google Gemini is not Chat-Completions-native.** It exposes a `generateContent`
   API with a different request/response shape. (Google does offer an OpenAI-compat
   endpoint, but routing through it forfeits Gemini-native features.) A native adapter,
   mirroring `AnthropicAdapter`, is the better long-term seam.

Two architectural blockers stand in the way of registering several OpenAI-compatible
providers at once:

- **`OpenAiCompatibleAdapter::provider()` hardcodes `LlmProvider::Mistral`.** The
  gateway routes by `HashMap<LlmProvider, Box<dyn LlmProviderAdapter>>`, so two
  OpenAI-compatible adapters registered together would collide on the single `Mistral`
  slot. (The TS adapter already carries a configurable `provider`; the Rust one does
  not.)
- **The `LlmProvider` enum** only has `Openai, Anthropic, Mistral, Ollama, Mock`. Each
  distinct provider needs its own enum slot to be routable. The enum is wire-compatible
  (serde `rename_all = "lowercase"`) with the TS `LLM_PROVIDERS` union.

## Decision

Extend the **Rust** gateway only (the live engine). The deprecated TypeScript
`@ailu-ai/llm-gateway` stays at its current two adapters — it is a fallback and is
not part of the supported execution path.

### 1. Provider enum

Add five variants to `LlmProvider` (`crates/llm-gateway/src/types.rs`):
`Google`, `Minimax`, `Openrouter`, `Huggingface`, `Lmstudio`. (`Openai` already exists
but was never wired to an adapter — it becomes real.) The enum keeps
`serde(rename_all = "lowercase")`, so the wire tokens are
`google | minimax | openrouter | huggingface | lmstudio`.

### 2. One configurable OpenAI-compatible adapter

`OpenAiCompatibleAdapter` gains a `provider: LlmProvider` field set at construction;
`provider()` returns it instead of the hardcoded `Mistral`. Named constructors are
added alongside the existing `mistral` / `ollama`:

| Constructor      | Provider slot | Base URL                              | Auth            |
| ---------------- | ------------- | ------------------------------------- | --------------- |
| `openai`         | `Openai`      | `https://api.openai.com/v1`           | `OPENAI_API_KEY`     |
| `openrouter`     | `Openrouter`  | `https://openrouter.ai/api/v1`        | `OPENROUTER_API_KEY` |
| `minimax`        | `Minimax`     | `https://api.minimax.io/v1`           | `MINIMAX_API_KEY`    |
| `huggingface`    | `Huggingface` | `https://router.huggingface.co/v1`    | `HF_TOKEN`           |
| `lmstudio`       | `Lmstudio`    | `http://localhost:1234/v1`            | keyless              |
| `mistral` *(kept)* | `Mistral`   | `https://api.mistral.ai/v1`           | `MISTRAL_API_KEY`    |
| `ollama` *(kept)*  | `Ollama`    | `http://localhost:11434/v1`           | keyless              |

The `looks_like_model_id` heuristic is adjusted so OpenRouter's slash-namespaced ids
(`openai/gpt-4o`) are treated as explicit model ids rather than routed to the default.

### 3. Native Gemini adapter

A new `crates/llm-gateway/src/gemini.rs`, structured like `anthropic.rs`: a pure
request-body builder + a `GeminiPort` transport seam (real `reqwest` impl + a fake for
tests, no network in tests). It maps the engine `LlmRequest` to Gemini's
`generateContent` body (system instruction, `contents` with `role`/`parts`, tool
declarations, `tool_use`/`functionCall` round-trip) and back to `LlmResponse`. Keyed on
`LlmProvider::Google`, auth via `GEMINI_API_KEY` (falling back to `GOOGLE_API_KEY`).

### 4. Model policy

`ModelPolicy::default` (`model_policy.rs`) gains per-tier model tables for the new
providers, and `available_from_env` learns their env keys (plus the
`AILU_USE_LMSTUDIO=1` flag for the keyless local server, mirroring the existing
`AILU_USE_OLLAMA` flag). Preference order is extended; hosted frontier providers
rank ahead of local ones.

### 5. Bindings plumbing (kept in mirror)

`crates/bindings/src/bridge.rs` and `crates/py-bindings/src/core.rs` are updated in
lockstep (a test already asserts they share the `available_from_env` path):

- `parse_provider` grows from 3 cases to cover all provider tokens.
- `build_gateway` registers the right adapter for the resolved provider, keyed off the
  provider's env credential; when credentials are absent it falls back to the
  deterministic mock under the resolved provider slot (unchanged offline guarantee).

## Consequences

- **No new wire breakage by construction:** adding enum variants is additive. Any code
  that pattern-matches `LlmProvider` exhaustively in Rust must handle the new arms
  (the compiler enforces this); the TS `LLM_PROVIDERS` union is *not* extended, so the
  TS fallback simply never sees the new tokens (it is not on the execution path).
- **Six providers, one adapter:** OpenAI, OpenRouter, MiniMax, Hugging Face, LM Studio,
  and the existing Mistral/Ollama all share `OpenAiCompatibleAdapter` — new providers
  of this family are henceforth a constructor + enum slot, not a new integration.
- **Gemini is a real native adapter,** carrying the same maintenance weight as
  `AnthropicAdapter` (its own wire mapping + tests), in exchange for access to
  Gemini-native request features later.
- **Offline determinism preserved:** with no credentials present, every provider still
  resolves to the scripted mock, so examples and tests run keyless.
- **TS fallback divergence is intentional and documented here:** the deprecated TS
  gateway keeps two adapters. If it is ever un-deprecated, a later ADR revisits parity.
