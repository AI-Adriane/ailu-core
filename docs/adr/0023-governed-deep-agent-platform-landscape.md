# ADR 0023 — Governed deep-agent platform: full capability landscape vs LangChain `deepagents`

- Status: Proposed (strategic landscape + phasing; no code commitment yet)
- Date: 2026-06-22
- Deciders: Mathieu (owner)
- Extends: [ADR 0022](0022-deep-agent-harness-gap.md) (core three: writeTodos / task / virtual fs)
- Related: [ADR 0013](0013-llm-council-governed-deliberation.md) (governed deliberation),
  [ADR 0014](0014-token-efficiency.md) (context budget / compression / caching),
  [ADR 0018](0018-model-tiering.md) (tiering → profiles),
  [ADR 0021](0021-opentelemetry-export-seam.md) (export seam pattern)

## Context

ADR 0022 scoped the *core three* deep-agent primitives (a planning tool, a sub-agent spawn
tool, a virtual filesystem). Reviewing the **full** LangChain deep-agent + agent-platform
surface — Skills, sandboxes/interpreters, context engineering, pluggable backends, async
sub-agents, permissions, memory, profiles, event streaming, a streaming frontend, prebuilt
middleware, and interop protocols (ACP / Google ADK) — shows a much larger landscape than 0022
captured. This ADR maps **all** of it against Ailu, decides **where each capability lives**
(engine primitive / external seam / control-plane / Studio product), and sets the **governed**
through-line and a phasing order. None of this surface exists in the engine today (verified): it
is greenfield beyond the existing reusable bricks (`principal` RBAC, `memory-store`, `EventBus`,
`LlmStreamChunk` type, `prebuilt-agents`, ADR 0014 compression/cache).

## Capability landscape

| Capability (deepagents / LangChain) | Ailu today | Gap | Lives in | Governed angle |
| --- | --- | --- | --- | --- |
| **writeTodos** (plan-in-state tool) | plan-execute pattern | agent-callable todo tool | `agents-core` | plan checkpointed + audited |
| **task** (sync sub-agent → compressed report) | subgraphs, supervisor, swarm, council | first-class spawn tool | `agents-core` | each spawn = audited node |
| **async sub-agents** (concurrent, stream back) | fan-out / `send` (parallel) | concurrent spawn + result streaming | `agents-core` + runtime | **human gate preserved** per spawn |
| **context engineering** (summarize/trim/offload) | working-memory compression, prompt caching + LLMLingua, `context_budget` (0014) | summarization-as-middleware | `agents-core` (extends 0014) | budget is config, not a prompt hack |
| **memory** (long-term, cross-thread) | `memory-store` (pluggable backends) | always-loaded `AGENTS.md` + cross-session wiring | `memory-store` | attributable, scoped store |
| **skills** (`SKILL.md` progressive) | KB / RAG + system-prompt path | the progressive-skill convention | KB + system prompt | skills versioned + governed |
| **virtual filesystem** (ls/read/write/edit/glob/grep) | `artifact-store` (versioned artifacts) | agent-operable fs | **`fs` seam** (0022 phase 2) | write/exec on guarded path → gate |
| **backends** (pluggable fs/state: local/store/composite) | `artifact-store` (single backend) | backend choice for the `fs` seam | `fs` seam | backend = policy decision |
| **sandbox / interpreters** (run code/shell) | — | isolated code execution | **external gated seam** (never in-engine) | exec **always** approval-gated |
| **permissions** (per-tool / per-path allow·deny·interrupt) | RBAC `principal`, ApprovalEngine, no-self-approval, signed audit | a per-tool/per-path rule DSL over tool calls | `agents-core` + `approval-engine` | **Ailu wins**: perms → gates + audit |
| **profiles** (preset agent configs) | `prebuilt-agents`, model tiering (0018) | named, shareable profile bundles | `agents-core` | profile = traced config |
| **prebuilt middleware** (summarization/HITL/PII/retry…) | scattered seams: `RedactingLlmGateway`, `CompressingGateway`, reflection | a **unified middleware composition API** | `agents-core` | governance becomes composable |
| **event streaming** (token / tool / sub-agent events) | `EventBus` (node lifecycle) + `LlmStreamChunk` (type, unwired) | wired streaming tool-transcript + SSE | runtime + bindings (follow-up) → API | streamed events stay audited |
| **frontend sub-agent streaming** | — | nested sub-agent stream UI | **Studio (product)** | — |
| **protocols (ACP / Google ADK)** | — | interop adapter (external clients drive/stream the agent) | **control-plane / API (seam)** | a governed agent, exposed via a standard |

## Decision

Adopt the deep-agent + agent-platform conveniences, but make Ailu's version **governed by
construction**: *every powerful action — filesystem write, code/shell execution, sub-agent spawn,
tool call — routes through permissions → an approval gate → signed audit → a checkpoint.* This is
the same bet as the council (0013): not a new trick, the **governed** version of a known-good
pattern. deepagents offers a bare `interrupt_on`; Ailu offers permissions + gates + audit +
sovereignty.

Placement rules (where each capability lives):

1. **Engine primitives (`agents-core` / seams)** — `writeTodos`, `task` (sync + async), the
   `fs` seam + pluggable backends, the permissions DSL, profiles, and a **middleware composition
   API** that unifies today's scattered seams (Redacting/Compressing gateways, reflection,
   summarization, HITL) into one composable surface. Skills/memory map onto KB + `memory-store`.
2. **External gated seams (never in-engine)** — **sandbox** and **interpreters** (code/shell
   execution). The OSS security rule forbids `eval`/exec of strings in-engine; execution is an
   external, approval-gated service behind an env-configured seam (the PII / LLMLingua / OTel
   pattern). `execute` always passes an approval gate.
3. **Control-plane / API seam** — interop **protocols (ACP, Google ADK)**: an adapter that lets
   external clients (editors, orchestrators) drive and stream a *governed* Ailu agent over a
   standard. Strategic interop, not an engine concern.
4. **Studio (product)** — the **frontend sub-agent streaming** UI consumes the engine's streamed
   events; rendering is a product feature, not engine.

The **unified middleware API** is the keystone abstraction: it turns each governance seam
(redaction, compression, HITL, summarization, retry, permissions) into a composable middleware,
so "governed deep agent" = a default middleware stack rather than bespoke wiring.

## Phasing

1. **Phase 1 (cheap, engine) — ✅ IMPLEMENTED.** `writeTodos` (a pure planning tool →
   durable `__todos` channel, sunk in the same checkpoint as the result) + `task`
   (`GraphBuilder.taskNode`, sugar over the existing subgraph node → checkpoint / audit /
   human-gate inherited, zero new runtime path). Parity Rust ↔ TS, additive wire
   (`AgentResult.todos` is optional), carrier wired on both the in-process and
   persisted/catalog paths. Adversarially reviewed (5 lenses); the only real finding — a
   pre-existing carrier gap also affecting the ADR 0014 knobs — was fixed in the same
   change.
2. **Phase 2 (the real build) — ✅ IMPLEMENTED ([ADR 0024](0024-governed-virtual-filesystem-seam.md), Accepted).** The governed `fs` seam (8 tools) + pluggable backends (artifact + durable HTTP, fail-closed) + the per-path permission DSL (`deny`<`read`<`gate`<`write`) → approval-gate integration with content-scoped grants. Phases 2a–2e shipped.
3. **Phase 3 (composition) — ✅ IMPLEMENTED ([ADR 0025](0025-unified-agent-middleware-api.md), Accepted).** The unified **middleware API**: one `AgentMiddleware` trait (7 hooks) + `MiddlewareStack` (governed/efficiency, onion); folded the scattered seams in (redaction, compression, terse, context-budget); the **approval gate is intrinsic** to `before_tool`; **profiles** (`fast`/`frontier-careful`/`governed-deep`) + user `middleware[]` ride on it (3d); **reflection** middleware (3e). Governed-by-construction: a governance kind in user data is unrepresentable.
4. **Phase 4 (async + streaming) — ✅ IMPLEMENTED ([ADR 0027](0027-async-subagents-and-streaming.md), Accepted).** 4a: incremental event streaming on the Rust engine (already shipped — `streamViaRust` projects forwarded events per node, all four `StreamMode`s). 4b: `mapAgents` dynamic fan-out (`map_node_handler` + `GraphBuilder.mapAgents`) — one sub-agent per item, concurrent, deterministic input-order merge, human-gate preserved per spawn. 4c (LLM token-delta streaming) deferred.
5. **Phase 5 (interop + UI)** — ACP/ADK protocol adapter (control-plane) + Studio sub-agent
   streaming UI.
6. **Phase 6 — skills/memory** — long-term cross-thread agent memory over `memory-store` (the four memory planes M1–M4, [ADR 0026](0026-memory-architecture-engine-studio.md), Proposed) + the progressive-skill (`SKILL.md`) convention over the KB. Best surfaced as a `MemoryMiddleware` (`before_run`/`after_run`) now that the middleware API exists.

### Beyond the deep-agent core — general-engine roadmap (the capability audit, 2026-06-23)

These came out of a deepagents/LangChain parity audit. Each is its own ADR + sign-off; all keep the governed-by-construction bet (a seam → policy → gate → audit → checkpoint, like the PII / fs / middleware seams). Numbering continues the platform roadmap:

7. **Phase 7 — observability (OTel + cost + dev-tools).** The `observability` crate has a `Tracer`/`MetricCollector` abstraction (in-memory only) but **no OTLP exporter** and it is **not documented**. Add an env-gated **OTLP exporter seam** (→ an OTel collector; LangSmith / Langfuse / Phoenix all consume OTLP), surface per-call token usage (`LlmUsage` already captures prompt/completion/cache) on `AgentResult` + the spans, and a **price-per-model → cost** mapping so a trace carries its cost. ⭐⭐⭐ (answers the Monitor gap: traces, cost, dev-tools).
8. **Phase 8 — structured output.** ✅ **Done** ([ADR 0029](0029-governed-structured-output.md)). Added a provider-neutral `LlmRequest.response_format`, fanned out per adapter (OpenAI `response_format`, Anthropic forced synthetic tool, Gemini `responseSchema`), plus an in-engine JSON-Schema **validation floor** (`jsonschema` crate) exposed as a user-installable `structuredOutput` efficiency-middleware kind (bounded deterministic retry, fail-closed / lenient modes). The validated value lands on `AgentResult.structuredOutput`; the gate stays intrinsic.
9. **Phase 9 — multimodal.** ✅ **Done** ([ADR 0030](0030-multimodal-input.md)). Additive `content_blocks` (text/image/audio/file) + `MediaSource` (artifact/url/base64) on `LlmMessage`/`LlmResponse` (text path byte-identical), per-adapter input fan-out (Anthropic/OpenAI/Gemini), PII-redaction of text blocks, the run entry surface (`inputBlocksChannel` → multimodal seed, 9e), ArtifactRef bytes resolution at the gateway boundary (`MediaResolver` + inline cap, 9c), and Gemini inline-media output parsing (9out). TS-adapter parity (9f) is N/A — there is no TS engine (Rust-only). Media *generation* (OpenAI/Anthropic separate APIs) = future seam.
10. **Phase 10 — secrets redaction + no-log.** ✅ **Done** ([ADR 0032](0032-secrets-redaction-and-no-log.md)). A deterministic in-engine `RegexSecretsRedactor` (governed, always-on, mask-default + `AILU_SECRETS_POLICY=block` opt-in via `LlmError::SecretsBlocked`) pushed **before** PII (+ optional external `AILU_SECRETS_REDACTOR_URL` augmentation), and a `ChannelDefinition.noLog` flag masked at the four `NodeCompleted` emit sites (sentinel `[REDACTED_NO_LOG]`) while **checkpointed in full** (durability ≠ observability). Known gap: free-text `NodeFailed.error`/`RunFailed.error`/`RunSuspended.reason` not covered.

### Deep-agent completion — explicit phases (expand the coarse phases 5 + 6 + 4c)

Phases 5 (interop + UI) and 6 (skills/memory) and 4c (token streaming) were coarse buckets; broken out as explicit phases (each its own ADR + sign-off, governed-by-construction):

11. **Phase 11 — long-term cross-thread memory.** 🟡 **Engine increment shipped** ([ADR 0026](0026-memory-architecture-engine-studio.md), GO 2026-06-24). New `ailu-memory` crate (unified **vector + entity-graph** recall + `MemoryProvenance` + `RetrievalPolicy` + `MemoryStore` seam + in-memory default) + a governed `MemoryMiddleware` (recall→inject seed / persist-after, sealed namespace+principal) wired via an agentNode `memory` overlay. **Control-plane follow-ups**: Neo4j persistence + native vector index (sub-phase A), governed LLM entity-extraction v2 (C), lifecycle/forgetting (E), BKP (F), Studio surfaces.
    Original gap (for reference): Missing impl: **M3** agent memory (`memory-store` `PgStore` is a stub that throws), **M2** pgvector (no semantic recall), **M4** portable export; and the wiring as a **`MemoryMiddleware`** (`before_run` loads relevant memories → injects; `after_run` persists) — clean now the middleware API exists. Plus scoping/governance (per-tenant/agent, attributable). The moat-#1 build.
12. **Phase 12 — skills (`SKILL.md` progressive disclosure).** ✅ **Done** ([ADR 0035](0035-skills-progressive-disclosure.md), shipped). `crates/skills` + `SkillMiddleware` (`crates/agents-core/src/skill_middleware.rs`) implement the format (frontmatter + body), progressive loading (load on demand, not always in-context), and the bridge/SDK/contracts wiring (`packages/contracts/src/skills.ts`); doc-site page + cookbook recipe (`governed-skills.md`) exist. Studio-side authoring surface (installing/browsing skills in the product) is tracked separately, product-side (issue #566 G5-era gap register in the private repo), not an engine gap.
13. **Phase 13 — token / tool / sub-agent event streaming.** ✅ **Done** ([ADR 0033](0033-token-streaming-and-spawn-tagging.md), signed off 2026-06-24). Per-**token** LLM deltas (`token_delta`, `crates/runtime-bridge/src/spec.rs` → `packages/graph-sdk/src/compiled-graph.ts`/`rust-engine.ts`/`run-catalog-graph.ts`) and nested sub-agent event tagging both shipped; cookbook recipe (`token-streaming.md`) exists.
14. **Phase 14 — ACP / Google ADK protocols.** Re-scoped (not this ADR's line item anymore): the private product repo's **ADR 0066** ("A2A agent interop") superseded this line — ACP (IBM/BeeAI) merged into A2A under the Linux Foundation and wound down, and Google ADK's interop story is native A2A, so "expose over ACP and ADK" collapsed to one real target. ADR 0066 is **Accepted** (ratified 2026-08-03), implemented control-plane-side (`product/apps/api/src/a2a/`: controller, service, delegate-tool, outbound-client, remote-agents, agent-card, task-lifecycle) — not an engine surface (the engine has no HTTP server). Nothing further to build here in `ailu-engine`.
15. **Phase 15 — Studio sub-agent UI.** Nothing (Studio = the commercial product). Missing: the nested sub-agent stream visualization, consuming phase-13 events. Product, not engine. (= part of the old phase 5.)
16. **Phase 16 — per-model provider packages + provider-agnostic SDK.** 🟡 **Foundation done** ([ADR 0031](0031-per-model-provider-packages.md)). Per-provider overlay packages (`@ailu-ai/model-{openai,anthropic,gemini,mistral}` + `openaiCompatible`) over the single Rust engine: a `Model` declares a serializable `ModelSpec` passed to `agentNode({ model })`, and is callable standalone via `Model.invoke()` (a one-shot through the Rust gateway over the napi `llmComplete` seam — LangChain DX, Rust execution). `agentNode.llm` is now optional+deprecated. **Follow-ups**: 16a slim `@anthropic-ai/sdk` out of the base; 16b docs/examples → `model()`; 16c publish config (flip `private`, versioning); **16d DX-beyond-LangChain** — `init("openai:gpt-4o")` string form, typed model-id catalog/autocomplete, typed `.invoke()` output, batteries-included defaults + great errors, *without losing performance* (owner request). (Supersedes the old "minor/cosmetic" note — this IS a capability + the open-core ecosystem story.)

> Also unphased (candidates): **sandbox / interpreters** (code/shell exec — an external, always-approval-gated seam, documented in integrations/sandboxes; never in the OSS engine per the security rule) and the **retry / rate-limit / tool-call-limit** middleware deferred from phase 3e (net-new efficiency/safety middleware on the existing hooks).

### Re-plan after phase 10 (2026-06-23; superseded — see phase-status correction below)

Status at the time: phases 1–10 + the phase-16 foundation were **shipped**. Re-prioritised order
for what remained, by value × cost:

- **Phase 11 — long-term memory** (moat #1, NEXT): the highest-value capability; ADR 0026 exists (needs GO). Biggest build — its own ADR/sign-off.
- **Phase 13 — token/sub-agent streaming**: enables phase 15 (Studio UI) and better DX; the napi token-stream bridge is the work. Prioritise over 12/14.
- **Phase 16 follow-ups (16a–16d)**: 16a (slim) + 16d (DX-beyond-LangChain) are cheap, high client-facing value — fold in opportunistically between bigger phases.
- **Phase 12 — skills** and **Phase 14 — ACP/ADK**: real but lower urgency; schedule after 11/13.
- **Phase 15 — Studio UI**: product (not engine); follows phase 13.

**Correction (issue #566 G11, 2026-08-03, re-verified against `origin/main`):** the plan above is
stale — phases 12, 13, and (re-scoped) 14 all shipped since this snapshot; see their own phase
entries above for current status. Phase 11 shipped its engine increment (ADR 0026); control-plane
follow-ups (Neo4j persistence, entity-extraction v2, lifecycle/forgetting, BKP, Studio surfaces)
are tracked product-side. Phase 15 (Studio sub-agent UI) remains product-side, not engine, and its
status isn't tracked in this ADR.

### Integrations taxonomy (the doc-site surface)

Every seam above is documented as a splittable **integration** (LangChain-style), so a reader picks a concrete piece: **models** (native Anthropic + Google Gemini; OpenAI-compatible for OpenAI / Azure / Mistral / OpenRouter / Groq / Hugging Face / Ollama / NVIDIA; AWS Bedrock via proxy/planned), **middleware**, **backends** (fs), **checkpointers**, **retrievers**, **text splitters**, **vector stores**, and **sandboxes** (external gated seam, planned). Adding a provider/seam of an existing family is a constructor + an enum slot, not a new integration (ADR 0005).

## Consequences

- A coherent product story: **"governed deep agents"** — the Claude-Code-shaped platform with
  permissions, approval gates, signed audit, RBAC, sovereignty, and the lightest orchestration
  (the benchmarks).
- The middleware API and the `fs` seam are the two big new abstractions; everything else is
  ergonomics over existing primitives or an external/edge seam.
- Engine stays lean: sandbox/interpreters/protocols never bloat the OSS core — they are seams.

## Reserves / next

- Large surface — do **not** build at once; the phasing is the contract. Phases 2 (fs+perms) and
  the sandbox/interpreter seam are security-relevant → each gets its own ADR + explicit sign-off
  before code (mandatory review).
- Open decision per 0022 stands: engine ships the primitives; the opinionated harness assembly is
  a prebuilt (recommended).
- ACP/ADK: pick one protocol to prototype first (ACP looks closer to the editor-client shape);
  ADK parity is a later interop bet.
- Async sub-agents must not bypass the human gate — the gate fires per spawn, even concurrent
  (the invariant that differentiates us; verify with a suspend/resume test under fan-out).
