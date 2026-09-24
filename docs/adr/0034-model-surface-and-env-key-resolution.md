# ADR 0034 — Unified `model` surface + env-key resolution (DX, phase 16a + 16d)

- Status: **Accepted** (signed off 2026-06-24). Public engine-package API + security-relevant key handling + removal of a silent default — all under mandatory human review; approved this session against the owner's bar: *the best DX of any agent framework, simple for an AI agent and every human alike*.
- Builds on / amends: [ADR 0031](0031-per-model-provider-packages.md) (per-model provider packages — this ships its deferred follow-ups 16a "slim @anthropic-ai/sdk out of base" + 16d "DX beyond LangChain"). Preserves [ADR 0020](0020-polyglot-sdk-seam.md) (methodless `ModelSpec`, one-shot napi) and reuses [ADR 0029](0029-structured-output.md) (`response_format` honored by `complete()`, so also by the one-shot `llmComplete`).

## Context

Picking a model today means importing a per-provider package and calling `openai("gpt-4o")` (factories from #59, not even re-exported from `graph-sdk`), or passing a bare `ModelSpec`. Two footguns: `agent-node.ts` defaults an unknown/absent provider to `"anthropic"` silently (`:465`,`:523`), and the standalone `invoke()` path ships `providerKeys: {}` (`rust-engine.ts:432`) so a real call has no key. The base `graph-sdk` also re-exports the deprecated TS-fallback gateway machinery (`index.ts:262-272`), which transitively pulls `@anthropic-ai/sdk` into every install even though provider calls only ever happen in the Rust engine.

## Decision

### 16a — Slim the base (breaking, sanctioned)
Remove from `graph-sdk`'s public surface the deprecated TS-fallback gateway re-exports (`DefaultLLMGateway`, `MockLLMProviderAdapter`, `AnthropicProviderAdapter`, `OpenAICompatibleProviderAdapter`, `InMemoryPromptRegistry`); keep the pure-data exports (`ModelPolicy`, `MODEL_TIERS`, `DEFAULT_TIER_TABLE`, tier types). Drop `@anthropic-ai/sdk` from `graph-sdk` deps + tsup `external`. In `llm-gateway` (the deprecated TS-fallback pkg) make `@anthropic-ai/sdk` an **optional** dependency, lazily required inside `anthropic-adapter.ts` (static `createRequire` of a constant package name — never a dynamic import of a user string). Result: `pnpm add @ailu-ai/graph-sdk` no longer pulls a provider SDK. Migration note: import the TS fallback from `@ailu-ai/llm-gateway` directly if you truly need it.

### 16d — One `model` surface (the DX bar)
A single import, `model` (alias `models`), a **callable namespace**:

- **Zero-config default** — `model.invoke(input)` ⇒ `ModelSpec { tier: "balanced" }`; the engine resolves the provider from whichever env keys are present, and throws `NoProviderInEnvError(checkedVars)` if none. Never mock, never silent-Anthropic.
- **Provider-as-method** — `model.openai("gpt-4o")`, `model.anthropic("claude-opus-4-8")`, … one uniform template across all providers; `model.cohere` is a **compile error** (not a key on the typed object).
- **Tier ladder as properties** — `model.fast`, `model.openai.fast`, `model.anthropic.frontier` (`fast|balanced|frontier|creative`).
- **Escape hatches** — `model.openaiCompatible({ baseURL, model, apiKeyEnv })` for any OpenAI-wire endpoint; object form `model({ provider, tier })` always accepted (so agents emitting JSON land correctly too); per-call `{ baseURL, apiKeyEnv }` override on the provider method.
- **Typed structured output** — `model.openai("gpt-4o").output(zodSchema).invoke(input)` returns `z.infer<S>`. The schema's JSON-Schema is serialized into the **single** `llmComplete` payload as `response_format`; the Rust engine drives provider-native structured output and validates (ADR 0029). TS only `z.infer`s the static type and `JSON.parse`s the returned content — no second bridge call, no TS HTTP client, no TS-side validator dependency (the schema arg is a generic `{ parse }` shape, not a hard Zod dep).

`model.<provider>(...)` / `model.fast` / `model({...})` all return a `Model` handle (methods hang off it); `Model.toSpec()` is the methodless `ModelSpec` that crosses napi/pyo3 (ADR 0020 intact). `agentNode({ model })` accepts `ModelLike = Model | ModelSpec | "provider:model"` and calls `toModelSpec()` — so a handle, a raw spec, and the colon-string are interchangeable, and the `?? "anthropic"` default is **deleted**.

### Env-key resolution (security)
One resolver in `model-core`, shared by the standalone `invoke()` and `agentNode` assembly: `resolveProviderKeys(spec)` reads `process.env[spec.apiKeyEnv ?? DEFAULT_KEY_ENV[spec.provider]]` (e.g. `openai→OPENAI_API_KEY`; `ollama`/`lmstudio`→none). Fail-loud: a named provider with no key throws `MissingProviderKeyError(provider, envVarName)`; zero keys on the tier-only/zero-config path throws `NoProviderInEnvError(checkedVars)`; an unknown provider throws `UnknownProviderError(provider, knownProviders)`. `apiKeyEnv` only ever names an **env var**, never a secret literal (CLAUDE.md security rule).

### Model-id catalog
Per-provider `as const` id tuples (the source of truth is the Rust adapter/`ModelPolicy`; shipped as small string arrays, regenerable — not a 200-member global `.d.ts` union). Each provider method is `model.openai(id?: OpenAiId | (string & {}))` — small per-call union for autocomplete, `(string & {})` keeps any new id valid without an SDK release. A test asserts the tuples are a subset of `DEFAULT_TIER_TABLE` to bound drift. Generating the tuples from Rust at build time is a later refinement (no codegen seam yet).

## Invariants
- **Zero runtime cost** — all sugar resolves to a plain `ModelSpec`; `invoke()` stays one `napi.llmComplete`. No TS HTTP client.
- **Methodless wire** — `ModelSpec` gains only the optional `apiKeyEnv` field (already in shape); no methods, pyo3 round-trip intact.
- **Fail loud** — unknown provider / missing key / no-provider-in-env all throw named errors; no silent default anywhere.

## Consequences
- Per-provider `model-*` packages (#59) keep their id tuples; their factories become thin spec-emitters surfaced under `model.<provider>` and re-exported from `graph-sdk`. The old factory exports stay as thin deprecated aliases for one minor (no hard delete — mandatory review).
- `@ailu-ai/graph-sdk` installs without any provider SDK.
- No new cross-language contract: `.output()` reuses the existing `response_format`; ADR 0020 unchanged.
