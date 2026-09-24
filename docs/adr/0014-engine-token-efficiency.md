# ADR 0014 — Token-efficiency features in the engine

- Status: merged to main + public PR #34. **Done + validated**: P1 (structured tool-call transcript,
  OpenAI-compat path), P2 (prompt caching — Anthropic emits `cache_control`; all three adapters read
  cached-prompt tokens into `LlmUsage`), P3 (terse output via `AgentNodeConfig.outputStyle`), P4
  (context-budget via `AgentNodeConfig.contextBudget`), and the **LLMLingua input-compression seam**
  (`compressor`, env-gated `AILU_LLMLINGUA_URL`, −44% input verified). **Native tool transcript
  complete across all three adapters**: OpenAI-compat (`tool_calls` + `tool` role), Gemini
  (functionCall/functionResponse, validated live), Anthropic (`tool_use`/`tool_result` content blocks —
  `AnthropicMessage.content` is now a JSON Value). `LlmMessage` carries `tool_calls` / `tool_call_id` /
  `tool_name`. The TS gateway already had content blocks. Whole engine green (38 cargo suites, clippy
  clean, napi rebuilt). **Remaining (non-material)**: streaming tool-transcript; TS `tool` parity is moot
  (TS engine is fallback-removed).
- Date: 2026-06-22
- Deciders: Mathieu (owner)

## Context

Measured token/latency benchmarks (see `benchmarks/`, dashboard of results) established two facts:

1. **The graph engine adds zero token tax** — on identical prompts, Ailu and LangGraph consume
   the same input tokens (e.g. product-flow 1197 = 1197). Orchestration is free, token-wise.
2. **The cost lives in the agent abstraction and the prompts**, and there are real, measured levers
   (proven out-of-engine in the benchmark harness):
   - **Trim inter-stage context** → −43% input (product-flow).
   - **Terse output** (caveman-style style directive) → −32% output, −31% latency (incident-postmortem).
   - **Prompt caching** of a repeated prefix → cached billed ~25% (−24% input cost on a 2 K-token prefix burst).

Separately, an audit of the Rust ReAct agent found a **robustness bug**: `LlmMessage { role, content }`
cannot represent a function-calling transcript (no `tool_calls`, no `tool` role / `tool_call_id`). The
agent re-injects a tool result as a plain `user` "observation:…" message, so on weaker models it does not
recognise the tool was answered and **re-calls it** → extra turns → extra tokens + latency.

This ADR proposes moving the proven levers **into the engine** so every Ailu agent/graph benefits,
and fixing the transcript bug.

## Decision (proposed)

Add four token-efficiency capabilities to the engine, phased by risk. Each is opt-in and preserves the
runtime invariants (determinism, checkpoint-after-every-node, events, human gates).

### P1 — Structured tool-call transcript (correctness + robustness) — **highest value**
- `LlmMessage` (crate `llm-gateway` + TS mirror) gains optional `tool_calls` (assistant) and a `tool`
  role with `tool_call_id`.
- `react.rs` (+ TS react-agent) replays a proper `assistant(tool_calls) → tool(result)` transcript
  instead of `user: "observation:…"`.
- Provider adapters (`gemini.rs`, `anthropic.rs`, `openai_compatible.rs`) serialize/deserialize the new
  fields in their native shapes.
- **Effect**: the model sees a coherent function-calling history → no redundant re-calls → ~2 turns
  instead of 2–4 on weak models → fewer tokens **and** lower latency (fewer round-trips). Also unblocks
  surfacing token usage in `AgentResult` (today it carries none).
- **Files**: `crates/llm-gateway/src/types.rs`, `crates/agents-core/src/react.rs`, the 3 adapters,
  `packages/llm-gateway`, `packages/agents-core`; napi rebuild.

### P2 — Prompt caching of the stable prefix (cost) — **biggest $ win on large/shared context**
- `gemini.rs` → `cachedContent`; `anthropic.rs` → `cache_control` blocks on the system prompt + tool
  defs (the prefix re-sent every turn / re-used across stages).
- Opt-in per agent/gateway (`cache: true` or a min-prefix threshold), surfaced in usage as cached tokens.
- **Effect**: repeated prefix billed ~10–25% → input **cost** down sharply on multi-turn agents, RAG,
  and pipelines that re-send a shared context. (Implicit Gemini caching exists but is probabilistic;
  explicit is reliable.)
- **Files**: `crates/llm-gateway/src/{gemini,anthropic}.rs`, gateway request types, config plumbing.

### P3 — Terse-output option (output tokens) — **cheap, opt-in**
- `AgentNodeConfig` / ReAct gain `outputStyle: "terse"` (or a `systemSuffix`), appending a compact
  output directive ("fragments, no filler, preserve substance/code/values").
- **Effect**: −15–32% output tokens + lower latency on **prose** outputs. Off by default; not for
  code-generation stages.
- **Files**: `crates/agents-core/src/react.rs` + `node.rs`, `packages/graph-sdk` `AgentNodeConfig`.

### P4 — Context budget (input tokens) — **wire the existing working-memory**
- Cap the `Input: {input}\nState: {state}` dump (today the agent serializes the **whole** channel map
  into its first message) to a configurable budget / selected channels, and wire the existing
  `working_memory` compression into the ReAct loop for long conversations.
- **Effect**: −input on multi-channel graphs and long agent loops.
- **Files**: `crates/agents-core/src/{react.rs,working_memory.rs}`, config.

## Consequences / reserves

- **Public-API surface**: `LlmMessage` shape, `AgentNodeConfig` options, gateway request fields change →
  versioned (minor) + TS/Python SDK mirrors + napi republish. This is why it is review-gated.
- **Determinism**: caching and terse-output do not change control flow; P1 changes the *message history*
  but not the graph — checkpoints/events/gates unchanged. To verify: re-run the fidelity + agent tests.
- **Measurement**: each lever is already measurable via `benchmarks/` (TERSE/TRIM flags, the token proxy)
  — implement P1→P4, re-run before/after, record the deltas here.
- **Order**: P1 first (correctness + robustness + unblocks usage reporting), then P2 (cost), then P3/P4
  (opt-in trims). Each is independently shippable.

## Status / next

Proposed for review. On GO, implement **P1 first** on a branch (engine change → re-run `cargo`/vitest +
the token benchmarks to show the turn/token reduction), then propose P2.
