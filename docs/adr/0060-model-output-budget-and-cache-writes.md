# ADR 0060: Model output budgets and provider cache writes

- Status: Accepted for this implementation scope by product decision 239 (Mathieu: « Oui, tout »).
- Product references: ADR-600 (output budget), ADR-429 (cache-write usage).

## Context

Anthropic already defaults to 16,000 output tokens in Rust. The OpenAI-compatible and
Gemini adapters leave the output budget to the provider. The deprecated TypeScript
Anthropic stream uses 64,000. OpenAI-compatible responses discard cache-write counts,
although `LlmUsage`, the SDK usage types and the ReAct accumulator already carry them.

## Decision

Every built-in generation adapter sends an output budget of 16,000 when `maxTokens`
is omitted, for complete and stream calls, including Mistral Conversations web search.
An explicit `maxTokens` replaces the default. Known lower model caps bound both the
default and explicit budgets. The cap is applied after the adapter resolves its model
fallback; it does not change the journaled request or replay matching.

The wire parameter follows the provider/model capabilities after model resolution:
OpenAI o-series (`o1`, `o3`, `o4-mini`, including dated/variant IDs) use
`max_completion_tokens` for both default and explicit budgets. Other OpenAI models
and compatible providers retain `max_tokens`; OpenRouter accepts that parameter even
for routed o-series models. See the [OpenAI Chat Completions contract](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
and [OpenRouter parameters](https://openrouter.ai/docs/api_reference/parameters).

The initial compatibility table is deliberately finite:

| Model IDs | Output cap |
| --- | ---: |
| `gpt-4`, `gpt-4-0613` | 8,192 |
| `gpt-4-turbo`, `gpt-3.5-turbo` | 4,096 |
| `claude-3-haiku-20240307` | 4,096 |
| `gemini-2.0-flash`, `gemini-2.0-flash-001` | 8,192 |

The same IDs with `openai/`, `anthropic/` or `google/` prefixes use the same caps on
OpenAI-compatible routing. Unknown model IDs get the requested budget or 16,000;
the provider enforces its own limits. Hosts using a model outside this table must
supply a lower `maxTokens` if it requires one. This is an output limit, not a context
window calculation: the engine does not estimate the remaining prompt capacity.

Cap references, checked on 2026-11-14:
[GPT-4](https://developers.openai.com/api/docs/models/gpt-4),
[GPT-4 Turbo](https://developers.openai.com/api/docs/models/gpt-4-turbo),
[GPT-3.5 Turbo](https://developers.openai.com/api/docs/models/gpt-3.5-turbo),
[Claude 3 Haiku](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-3-haiku.html),
[Gemini 2.0 Flash](https://ai.google.dev/gemini-api/docs/models/gemini-2.0-flash).
These historical caps do not assert that a model is still available.

OpenAI-compatible Chat Completions map
`usage.prompt_tokens_details.cache_write_tokens` to `usage.cacheWriteTokens`.
`usage.cache_creation_input_tokens` is accepted as an Anthropic-shaped fallback.
When both exist, the nested count wins, including an explicit zero. The existing
`cached_tokens` read count follows the same response path. Streaming uses the final
provider usage chunk and the same mapping. See the provider's
[usage contract](https://github.com/OpenRouterTeam/docs/blob/main/api_reference/overview.mdx).

No cache-write count is derived from input size, a cache miss or a cache-read count.
Missing OpenAI-compatible counts remain absent; Anthropic's existing absent-to-zero
mapping stays unchanged. Gemini's native response has no cache-write mapping here.
Mistral Conversations usage remains unchanged because it has no mapped cache counters.

The public SDK cost estimator also retains an explicit cache-write convention in
`ModelPrice.promptIncludesCacheWrites`: `true` for inclusive OpenAI-compatible prompt
counts, `false` for Anthropic's separate counts. Included reads and writes are removed
from the ordinary input bucket before applying their own rates and counting priced
tokens for coverage. An unspecified convention with reported writes is `estimated`,
as is a combined inclusive cache count larger than the prompt. Existing price books
remain valid; callers must state the convention to obtain a `known` cached cost.

## SDK parity and product upgrade

The behavior lives in the Rust gateway, reached by the TypeScript napi and Python
pyo3 model/graph paths through the shared runtime bridge. Both return the serialized
`LlmUsage` object. `ModelUsage`, graph result usage and `LLMResponse.usage` already
include optional `cacheWriteTokens`; no renamed field or required field is introduced.
The deprecated TypeScript provider adapters mirror the default and usage mappings.

Propose **2.9.0** as the next minor release from 2.8.5. This lot does not bump manifests,
publish artifacts or authorize a release. The release owner must update the public
SDK, native platform packages and Python version together, following `RELEASING.md`.

After that release, the product bump PR must:

- Pin the released SDK and native artifacts together, and check the actual loaded engine version.
- Keep `cacheWriteTokens?: number` in response validation, event projections, persisted usage
  and billing input contracts, wherever `cacheReadTokens` is already retained.
- Read provider counts for each call and aggregate once. Preserve absence as unreported
  where the product distinguishes unknown from zero. Do not infer writes from misses.
- Check billing rules before adding cache counts to prompt counts: OpenAI-compatible
  prompt usage can already include cached tokens. This change reports counts and does
  not redefine billing or token totals.
- Remove an obsolete caller default if it should inherit 16,000, or keep an explicit
  budget where a product flow needs another value. Qualify lower-cap models and
  custom endpoints with their actual limits.
- Exercise complete and streamed usage, reported zero, absent counters, and legacy
  responses written before the release. The product changes are outside this lot.
