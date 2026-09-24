# ADR 0025 — Unified agent middleware API (deep-agent platform phase 3)

- Status: Proposed (structural keystone — design for sign-off; **no code until approved**)
- Date: 2026-06-23
- Deciders: Mathieu (owner)
- Implements: [ADR 0023](0023-governed-deep-agent-platform-landscape.md) **phase 3** (the keystone)
- Folds in: [ADR 0008](0008-pii-redaction-and-anonymization.md) (redaction seam), [ADR 0014](0014-token-efficiency.md)
  (terse/trim/compression), [ADR 0018](0018-model-tiering.md) (tiering → profiles), [ADR 0019](0019-reflection-structured-critique.md)
  (reflection), [ADR 0024](0024-governed-virtual-filesystem-seam.md) (the approval gate + fs policy)
- Bet: same as [ADR 0013](0013-llm-council-governed-deliberation.md) — the **governed** version of a known-good pattern
  (LangChain agent middleware), not a new trick

## Context

Ailu's per-agent governance/efficiency behaviours are scattered across `bridge.rs::build_agent_handler`
as bespoke wiring: `wrap_with_compressor(wrap_with_redactor(build_gateway(...)))` decorator nesting,
a terse system-prompt suffix, a `context_budget` trim, the approval gate inline in `react.rs::execute_tool_call`,
the fs path-policy check, the `writeTodos` sink, reflection as a separate agent. Each new capability adds
another knob and another wiring line. There is no single composition surface, no way for a user to add a
behaviour, and no explicit, auditable ordering.

LangChain shipped an **agent middleware** model (built-in middlewares for summarization, HITL, PII, retry,
context-editing, …) with named lifecycle hooks (`before_model`/`after_model`/`before_tool`/…) composed into a
stack. ADR 0023 named the unified middleware API the **keystone** of the governed deep-agent platform: it
turns each seam into a composable middleware, so "governed deep agent" becomes a **default middleware stack**
rather than bespoke wiring.

## Decision

Introduce **one `AgentMiddleware` trait** with seven optional async hooks (all pass-through by default),
composed into one ordered **`MiddlewareStack`** that the ReAct loop drives. Adopt the LangChain named-hook
model but make it **governed by construction**.

### 1. Unify UP into the agent loop (the central decision)

The seams split today into **gateway-level** (wrap `gateway.complete()`: redact, compress) and
**loop-level** (`react.rs`: terse, trim, approval, reflection, todos). Rather than two mechanisms, **the
agent loop is the single composition spine.** `ReActAgent::run` already owns the only call to
`gateway.complete()`, so the gateway seams stop being a wrapper chain and become **request/response hooks on
the same stack**, fired around that call.

- The bare `LlmGateway` trait + `DefaultLlmGateway` router stay **unchanged** (a dumb provider router).
- `RedactingGateway` / `CompressingGateway` are **reborn as `RedactMiddleware` / `CompressMiddleware`** on the
  hooks. The structs are **kept** for non-agent callers (control plane, direct gateway users) — not deleted
  (accepted duplication for back-compat).
- **Why up, not down:** terse (system edit), trim (state injection), approval (tool gate + loop break),
  reflection (loop re-entry), todos (sink) all need loop state the gateway never sees (conversation,
  iteration, `approved_tool_names`, the registry, the break decision). The gateway sees one `LlmRequest`; the
  loop is the superset. So redact/compress migrate up cleanly; nothing migrates down.

### 2. Seven hooks (onion semantics), fired by `ReActAgent::run`

| hook | when | folds in |
| --- | --- | --- |
| `before_run` | once, after state injection, before the loop | `ContextBudget` trim; input validation / short-circuit |
| `before_model` | in-loop, just before `gateway.complete()` — **fallible** (Err short-circuits, fail-closed) | `Redact` scrub, `Compress` user-msg shrink, `Terse` system edit |
| `after_model` | after `complete()` returns, before parse | `Redact` hydrate; response guardrails |
| `before_tool` | in `execute_tool_call`, before execution | the **approval gate** (→ `Gate`), **fs policy** (→ `Deny`) |
| `after_tool` | after the handler returns, before the observation push | `Todos` capture |
| `on_iteration` | each loop-turn end | loop-detection, budget, reflection trigger (not the hard `max_iterations` bound) |
| `after_run` | after the loop, before `AgentResult` | finalize `requiresHumanReview`, `Reflection`, metadata |

**Ordering = declaration order, made explicit.** Before-hooks run outer→inner (index 0 outermost on the
request path); after-hooks run inner→outer. So `RedactMiddleware` at index 0 redacts **first** and hydrates
**last** — preserving today's `RedactingGateway`-wraps-`CompressingGateway` nesting, instead of it being
buried at `bridge.rs:466`.

### 3. The trait (Rust, `crates/agents-core/src/middleware.rs`)

`AgentMiddleware: Send + Sync` (`async_trait`), every hook a pass-through default so an impl overrides only
what it needs:

- `name()`; `before_run(&mut RunCtx) -> Result<Flow>`; `before_model(LlmRequest, &RunCtx) -> Result<LlmRequest>`;
  `after_model(LlmResponse, &LlmRequest, &RunCtx) -> Result<LlmResponse>`; `before_tool(&ToolCallCtx, &RunCtx)
  -> Result<ToolControl>`; `after_tool(name, &input, ToolResult, &RunCtx) -> Result<ToolResult>`;
  `on_iteration(index, &content, &RunCtx) -> Flow`; `after_run(&mut AgentResult, &RunCtx) -> Result<()>`.
- `Flow = Continue | Stop { reason }`. `ToolControl = Allow { override?: Value } | Deny { reason } |
  Gate(ApprovalRequestItem)`. `RunCtx` = a cheap per-call read-only snapshot (conversation slice,
  `approved_tool_names`, iteration index, channels) — built per hook-call, never held across a mutation
  (avoids the borrow-checker fight).
- `MiddlewareStack { governed: Vec<Arc<dyn AgentMiddleware>>, efficiency: Vec<…> }`. Request path =
  governed→efficiency; response path reversed. `ReActAgent` gains one field (default empty = today's
  behaviour) + a `with_middleware` builder. `async_trait + Arc<dyn>` matches the existing `PiiRedactor` /
  `PromptCompressor` (object-safe). `before_model`'s `Result` shape is **exactly** `PiiRedactor::redact_request`,
  so the fold is mechanical.

### 4. Governed / efficiency split — a TYPE invariant (governed by construction)

The governed layer (`Redact`, `ApprovalGate`, `FsPolicy`) is **builder-injected, sealed, and un-removable**;
the efficiency layer (`Compress`, `Terse`, `ContextBudget`) is user-tunable. Only the builder fills
`governed`; SDK users append only to `efficiency` (and the builder **rejects governance kinds** from user
middleware). **You cannot express an ungoverned stack** — that is the council bet, enforced by the type.

### 5. The default governed stack (a governed deep agent IS this stack)

Request-path order, outermost first. **Governed (sealed):** `RedactMiddleware` (fail-closed on `PiiBlocked`;
Noop sentinel when `AILU_PII_REDACTOR_URL` unset) → `ApprovalGateMiddleware` (**always present**,
non-negotiable HITL, no self-approval) → `FsPolicyMiddleware` (fail-closed `Deny`, when `enable_fs`).
**Efficiency (user-tunable, inner):** `CompressMiddleware` (when LLMLingua URL set) → `TerseMiddleware`
(`output_style: terse`) → `ContextBudgetMiddleware` (`before_run`, when `context_budget` set). **Built-in
(always, but inspectable):** `TodosMiddleware`, `ReflectionMiddleware` (opt-in). Efficiency strictly *inside*
governance, so compression never alters what the gate or redactor saw. This is `DEFAULT_GOVERNED_STACK`.

### 6. Profiles (ride the stack; ADR 0018 + 0023)

A `Profile` is a **named data bundle**: `tier` (→ `ModelPolicy`, unchanged), an efficiency-middleware vec, a
suspend default. The governed layer is identical across all profiles — **you cannot buy out of the gate**.
`fast` = tier fast + Compress/Terse/ContextBudget(4000), no suspend; `frontier-careful` = tier frontier +
ContextBudget(16000) + Reflection(2) + suspend, no compression; **`governed-deep`** (the keystone preset) =
tier balanced + full efficiency + Reflection + suspend + `enable_fs` — the governed deep agent as a one-liner.
Profiles are pure data (a registry of name→`ProfileSpec`), never code (mirrors the `ConditionRegistry`
never-eval rule). Tier picks the model; profile picks the middleware; `tierOverride` still wins.

### 7. SDK surface (TS, data-only — the one-engine invariant)

`AgentNodeConfig` gains two additive fields: `profile?` and `middleware?: MiddlewareSpec[]` (efficiency-layer
entries, **data** `{ kind, params }` — never a TS function, so it serialises to `RustAgentConfig` and the
**Rust** engine builds the real middleware). The builder validates kinds and **rejects governance kinds**
(`approvalGate`/`redact` are engine-injected) — the SDK expression of governed-by-construction. `outputStyle`
and `contextBudget` **stay flat knobs** (back-compat) but become **sugar** the builder desugars into
`Terse`/`ContextBudget` middleware. Precedence: profile expands → flat-knob sugar → explicit middleware (most
specific wins). `toRustAgentConfig` emits a `resolvedMiddleware` (the final ordered efficiency list); the
governed layer is **not** serialised (the engine injects it).

## Phasing (each step is non-breaking and independently shippable)

- **3a** — `middleware.rs`: trait + `Flow`/`ToolControl`/`RunCtx`/`MiddlewareStack`, all hooks pass-through;
  wire the seven fire-sites into `ReActAgent::run` guarded by a **default-empty** stack → **zero behaviour
  change**. The parity anchor (`suspends_for_approval_then_executes_after_grant`) must pass unchanged.
- **3b** — fold the two gateway seams (`Redact`, `Compress`) onto the request/response hooks behind the
  existing env vars; `bridge.rs` builds stack entries instead of `wrap_with_*`.
- **3c** — fold the loop seams behind the existing knobs: `Terse`, `ContextBudget`, **`ApprovalGate`** (the
  gate moves from `execute_tool_call` into `before_tool` — **highest-stakes**, must reproduce the
  content-scoped key + loop-break + no-self-approval exactly), `Todos`.
- **3d** — SDK: `profile` + `middleware` on `AgentNodeConfig`/`AgentSpec`, the profile registry, the
  desugaring, the governance-kind rejection; wire prebuilt presets to profiles.
- **3e** (net-new, optional) — `ReflectionMiddleware` (`after_run`), then the ADR 0023 wishlist the seam now
  enables: per-tool retry, rate-limit, tool-call-limit.
- **Deferred:** deprecating the flat knobs (terse/contextBudget stay permanent sugar in phase 3 — a later ADR).

## Key decisions (confirm before code)

1. **Unify up into the loop** — the agent loop is the single composition spine; redact/compress migrate onto
   request/response hooks (one mechanism, not two). *This is the whole ADR.*
2. **Keep** the bare `LlmGateway` + `RedactingGateway`/`CompressingGateway` structs for non-agent callers (the
   agent path stops using them; they are not deleted) — accept the duplication for back-compat.
3. **Governed/efficiency split as a type invariant** — gate + redaction + fs-policy are sealed and
   un-removable by users; SDK middleware rejects governance kinds.
4. **Flat knobs stay** — `outputStyle`/`contextBudget` remain permanent sugar; defer deprecation.
5. **Approval semantics unchanged** — the gate stays loop-terminal (first gated tool breaks the run, one
   approval per run); no batch-approval in phase 3.
6. **Reflection** — add the middleware form (`after_run`) **without** removing the standalone graph-node form
   (or pick one).
7. **Profile table** — confirm the concrete presets (`fast`, `frontier-careful`, `governed-deep`) before encoding.
8. **Dispatch** — `async_trait` + `Arc<dyn AgentMiddleware>` dynamic dispatch (matching `PiiRedactor`) is acceptable.

## Risks

- **Approval-gate move (highest-stakes)** — relocating the `requires_approval` check from inline
  `execute_tool_call` into `before_tool` must reproduce the content-scoped `approval_key`, the
  `ToolOutcome::Approval` loop-break, and no-self-approval **exactly**; the existing suspend/resume test is the
  parity anchor and must pass byte-for-byte.
- **Ordering regression** — redact-before-compress is struct nesting today; folding into an ordered vec risks
  reversing semantics → a parity test asserts redact-then-compress.
- **Fail-open vs fail-closed must survive** — `Redact` `before_model` is fail-closed (Err short-circuits);
  `Compress` is fail-open (must swallow internally, never Err). The unified `Result` hook supports both.
- **Perf** — per-iteration hook fan-out adds vtable + async overhead around each `complete()`; negligible vs
  LLM latency, but keep the default stack shallow.
- **TS↔Rust drift** — profiles/desugaring in TS, execution in Rust; a `.rust.test.ts` asserts the mapping
  through the bridge so a profile can't silently no-op.
- **Streaming gap** — no `stream()` on `LlmGateway` today; the design is unary-only, but the trait must be
  shaped so `before_stream`/`after_chunk` are additive later (say so).
- **`RunCtx` borrows** — build it per hook-call (cheap snapshot), never hold across a loop mutation.
- **Scope creep** — retry/rate-limit/circuit-break don't exist yet; phasing puts net-new middleware in 3e,
  after the trait + existing-seam folds are proven non-breaking.

## Consequences

- "Governed deep agent" becomes `DEFAULT_GOVERNED_STACK` + a profile — a one-liner, not bespoke `bridge.rs`
  wiring. The composition order is explicit + auditable instead of buried in decorator nesting.
- Governance is enforced by the type system (un-removable gate/redaction/fs-policy), not by convention.
- The runtime determinism/checkpoint contract is **untouched** — middleware runs *inside* a single node's
  `agent.run()` and never spans node boundaries (this is why it is loop-internal, not runtime-level).
- New cross-cutting behaviours (retry, rate-limit, summarization) become a middleware, not another `bridge.rs`
  knob — the seam pays for itself from 3e on.
