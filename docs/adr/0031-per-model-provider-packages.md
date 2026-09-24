# ADR 0031 — Per-model provider packages + provider-agnostic base SDK

- **Status**: Accepted (design signed off 2026-06-23; owner directive + decisions D1–D5)
- **Context**: SDK packaging / public API. Builds on [ADR 0016](0016-rust-only-sdk-no-ts-fallback.md) (Rust-only execution, no TS engine) and [ADR 0020](0020-polyglot-sdk-seam.md) (one Rust engine + thin napi/pyo3 SDKs).

## Context

Today every agent node must carry an LLM gateway: `AgentNodeConfig.llm: LLMGateway` is **required** (`packages/graph-sdk/src/agent-node.ts:47`), even though `provider`/`model`/`tier` are what actually steer routing. The TS `packages/llm-gateway` is monolithic and **already `@deprecated`** (`packages/llm-gateway/src/index.ts:1-8`): provider calls route through the Rust `crates/llm-gateway` via `@ailu/napi`.

Hard constraint (grounded): **execution is Rust.** `llm` is provably dead on the Rust path — `toRustAgentConfig` never reads it (`agent-node.ts:482-511`), `RustAgentConfig`/`AgentSpecWire` have no `llm` field (`agent-node.ts:293-339`, `rust-engine.ts:359-377`), and Rust's `AgentSpec` has no gateway field (`crates/bindings/src/spec.rs:16-29`). `config.llm` is consumed only by the now-dead TS fallback (`createAgentNodeHandler` → `new ReActAgent`, `agent-node.ts:650`) and `streamAgentTokens` (`agent-node.ts:547-556`). The entire provider-selection input Rust receives is a provider **string** + `tier` + the `providerKeys` map (`rust-engine.ts:143-156`), resolved by `key_for(slug, ENV)` (tenant-key-first-then-env, `bridge.rs:877-883`). The adapter set is **compiled into the binary** (`parse_provider` `bridge.rs:1025-1039`, `build_gateway` `bridge.rs:862-977`).

Owner directive (verbatim): *« il faut qu'il y est un packages par model plutôt qu'un llm-gateway … optionnel dans le SDK de base mais qu'ils puisse être ajouter tout de même @ailu/model-openai / gemini / anthropic / mistral »*, and *« je veux un moteur rust et dans le sdk des surcouches »* — i.e. **one Rust engine + thin per-provider SDK overlays**, like LangChain JS's per-provider packages (`@langchain/openai`).

**The reconciliation (answering "why can't I have the same as LangChain?").** LangChain JS packages *contain* the HTTP client and execute in JS. Ailu keeps **one Rust engine**; the per-provider packages are **thin overlays (surcouches)** — no engine, no duplicated HTTP client. They (a) declare config passed to `agentNode` (Rust executes the graph), and (b) optionally expose `.invoke()/.stream()` that call a **one-shot through the Rust gateway via napi**. Same authoring DX as LangChain; the HTTP happens in Rust (one engine, consistent behaviour across SDK/graph/standalone), not in JS.

## Decision

### 1. A `Model` overlay class per provider (D1)
Ship `@ailu/model-openai`, `@ailu/model-anthropic`, `@ailu/model-gemini`, `@ailu/model-mistral`, plus an `openaiCompatible({ baseURL, … })` escape-hatch helper. Each exports a thin **class** (the owner's choice) over a shared `Model` base:

```ts
import { OpenAIModel } from "@ailu/model-openai";
const m = new OpenAIModel("gpt-4o");        // or OpenAIModel.frontier()
m.toSpec();                                  // → ModelSpec { provider:"openai", model:"gpt-4o", … } (serializable)
await m.invoke([{ role:"user", content:"hi" }]);  // one-shot via napi → Rust gateway
```

The class is a **surcouche**: it carries `{ provider, model?, tier?, baseURL?, apiKeyEnv? }`, serializes to a plain `ModelSpec` via `toSpec()`/`toJSON()` (so it round-trips through napi **and** pyo3, ADR 0020), and its `invoke`/`stream` delegate to the Rust engine. **No HTTP client, no provider SDK** ships in these packages (except possibly `@anthropic-ai/sdk` relocating into `model-anthropic` — see §5). Packages carry only factories + model-id constants; `ModelPolicy`/tier tables stay single-sourced in the base (no drift from the Rust `ModelPolicy`).

### 2. Base SDK becomes provider-agnostic (D2)
`AgentNodeConfig.llm` → **optional + deprecated** (no-op on Rust); add optional **`model?: Model | ModelSpec`**. `agentNode` reads `provider`/`model`/`tier` from the `ModelSpec` (`m.toSpec()`); `toRustAgentConfig` stays `llm`-free on the wire. `provider`/`model`/`tier` flat fields remain accepted as deprecated aliases for one cycle. Existing `llm: new DefaultLLMGateway()` callers keep compiling. When neither `model`/`provider`/`tier` nor a `profile` is set, **warn loudly** (defuse the silent-`anthropic` default at `agent-node.ts:493` / `bridge.rs:1037`); an unknown provider slug **fails loudly** rather than silently becoming Anthropic.

### 3. Standalone `.invoke()/.stream()` via a napi one-shot (D4)
Add a napi function `llmComplete(requestJson, providerKeysJson) → responseJson` (and a streaming variant) that builds the Rust gateway (`build_gateway`, reusing `key_for` env/tenant keys + the media resolver) and calls `complete`. The `Model` class's `invoke`/`stream` call it. This gives LangChain-parity standalone DX **without** a TS HTTP client or a second engine — the HTTP is Rust.

### 4. The Rust engine is UNCHANGED
Compiled-in adapters + routing by provider string (`parse_provider` → `build_gateway` → `key_for`) are untouched. A `ModelSpec.apiKeyEnv`/`baseURL` maps onto the existing `providerKeys` channel + `OpenAiCompatibleAdapter` constructor args. No new Rust provider arm, no wire change for the graph path.

### 5. Slim the base SDK (D5)
The only real bloat is `@anthropic-ai/sdk`, pulled in solely by the inlined `anthropic-adapter` (`graph-sdk/package.json:43`). Relocate the dead TS HTTP adapters (`anthropic-adapter`, `openai-compatible-adapter`) out of the base — `anthropic-adapter` → `model-anthropic` (the only `@anthropic-ai/sdk` consumer), the fetch-based `openai-compatible-adapter` logic → the model-* packages' `invoke` path / removed. Drop `@anthropic-ai/sdk` from `graph-sdk` hard deps and the tsup-inlined surface. Keep `DefaultLLMGateway`/`MockLLMProviderAdapter`/`ModelPolicy`/`InMemoryPromptRegistry`/types reachable for the in-tree fallback + offline examples **this cycle**; full removal of the adapter re-exports from `graph-sdk/src/index.ts` = a **follow-up breaking change** once docs/examples are migrated to `model()`.

## Honest framing (D5)
A model package is an **authoring-ergonomics + model-id-catalog + credential-declaration overlay**, *not* new execution. The adapter set is fixed at `cargo build`; a package maps a provider slug + model id + endpoint + credential onto an **already-compiled-in** adapter. The genuine extension point for an unknown endpoint is `openaiCompatible({ baseURL })` (the OpenAI-compatible Rust adapter). "Add a provider" = config/registration, not a new Rust adapter (that needs a cargo build). This is stated plainly so the ecosystem story stays truthful.

## Alternatives considered
- **One `@ailu/models` package** — simplest/one version, but weaker ecosystem story + no per-provider opt-in. Rejected per the owner's per-package directive.
- **Per-package TS HTTP adapters that execute in JS** — re-introduces a TS execution path; contradicts ADR 0016 + "plus de moteur TS". Rejected.
- **Terse `@ailu/openai`** — squats vendor names + implies a vendor SDK. Rejected for `model-<provider>` (honest, parallels the per-provider doc pages).
- **Keep the monolith + required `llm`** — rejected by directive; keeps `@anthropic-ai/sdk` a base dep for an adapter that never runs.

## Consequences
- **DX parity with LangChain**: `new OpenAIModel("gpt-4o")` is passed to agents AND callable standalone (`.invoke`), via the Rust engine — one engine, consistent behaviour.
- **Back-compat**: `llm` optional+deprecated → existing callers + docs compile; keyless deterministic-mock keeps offline examples/tests running.
- **Slimmer base**: a base-SDK install no longer pulls `@anthropic-ai/sdk` (the concrete payoff). Dropping adapter re-exports is deferred (breaking) to next cycle.
- **Polyglot**: `ModelSpec` is plain data → the same authoring helper works from the Python SDK (pyo3), and `.invoke()` works there too via the same napi/pyo3 one-shot.
- **Cost**: N new publishable packages (Turbo auto-discovers `packages/*`; each needs `publishConfig`/`files`/`prepublishOnly` + a `tsconfig.base.json` path alias). Fixed lock-step versioning (current 1.2.0).
- **Footgun closed**: unknown provider slug now fails loudly instead of silently routing to Anthropic.

## Phasing & delivery status

**Shipped (this PR):** 1, 2, 3, 4 — the new architecture is usable end-to-end (`agentNode({ model })` on Rust, `Model.invoke()` standalone via napi, the 4 provider packages + `openaiCompatible`).
**Follow-ups (named):** 5 (slim `@anthropic-ai/sdk` out of the base), 6 (docs/examples migration), 7 (publish config + versioning), **8 (DX beyond LangChain — owner request)**.

- **1** ✅ `@ailu/model-core`: `ModelSpec {provider, model?, tier?, baseURL?, apiKeyEnv?}` + `Model` base (`toSpec()`/`toJSON()` + `invoke()`/`stream()`) + `assertKnownProvider` (fail-loud) + `openaiCompatible`. Round-trips through napi/pyo3.
- **2** ✅ napi `llmComplete(requestJson, providerKeysJson)`: factored `register_provider_adapter` + `build_standalone_gateway` in the bridge; `Model.invoke` delegates over the napi seam.
- **3** ✅ base SDK API: `agentNode` `llm?` optional + `model?: string | ModelLike`; `toRustAgentConfig` reads provider/model/tier from the overlay (the flat aliases stay deprecated). No Rust graph-path change. TS-fallback handler defers its `llm` check to call time so `agentNode({ model })` builds cleanly.
- **4** ✅ `@ailu/model-{openai,anthropic,gemini,mistral}`: a `Model` subclass + an `xxx()` factory with `.frontier()/.balanced()/.fast()` tier shortcuts; path aliases + per-package vitest config; re-exported from `graph-sdk`.
- **5** ⏳ slim: relocate `anthropic-adapter` → `model-anthropic`; drop `@anthropic-ai/sdk` from `graph-sdk`; keep `DefaultLLMGateway`/mock reachable. (Follow-up.)
- **6** ⏳ docs + examples migrate to `new OpenAIModel(...)` / `model()`; per-provider doc pages. (Follow-up.)
- **7** ⏳ publish config (publishConfig/files/prepublishOnly) + versioning; flip packages off `private`; confirm base install no longer pulls `@anthropic-ai/sdk`. (Follow-up.)
- **8** ⏳ **DX beyond LangChain (owner request)** — push the authoring DX past LangChain's in level + ease *without* losing performance: e.g. `init("openai:gpt-4o")` string form, smart model-id autocomplete/typed catalog, one-import ergonomics, structured `.invoke()` returning typed output, batteries-included defaults, great errors. Scoped + grounded in its own pass. (Follow-up.)
