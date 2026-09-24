---
sidebar_position: 4
title: Errors
description: Every typed TypeScript error and interrupt class, the Python ValueError subclasses, and the Result discriminated union.
---

# Errors

Ailu never throws a bare `Error("…")` from its own code: every failure mode is a typed class
you can `instanceof`-check and handle precisely. This page lists each error and interrupt class,
when it is thrown, and how to handle it — plus the Python `ValueError` subclasses and the
`Result` discriminated union returned by `safeCompile`.

## TypeScript: SDK errors

These come from `@ailu-ai/graph-sdk` (`packages/graph-sdk/src/errors.ts`). All extend the
common base `AiluSdkError`, so a single `catch (e) { if (e instanceof AiluSdkError) ... }`
covers them all.

### `AiluSdkError`

The base class for every error thrown by the SDK. Not thrown directly — catch a subclass, or
catch this to handle any SDK error generically.

### `GraphCompileError`

Thrown by [`builder.compile()`](/docs/reference/builder-api#compile) when the graph fails
validation. Carries `errors: GraphValidationError[]` — the full list of structural problems —
and a message summarizing each as `code: message`.

| | |
| --- | --- |
| **When** | `compile()` is called on a graph that does not validate (missing entry node, dangling edge reference, a cycle the recursion model rejects, etc.). |
| **Handle** | Prefer [`safeCompile()`](#the-result-discriminated-union) to avoid the throw; or `catch` and inspect `error.errors` (each has `code`, `message`, `path`). |

```ts
try {
  createGraph({ name: "x" }).compile();
} catch (error) {
  if (error instanceof GraphCompileError) {
    for (const v of error.errors) console.error(v.code, v.message, v.path);
  }
}
```

Expected result: logs `MISSING_ENTRY_NODE` (the graph declared no nodes).

### `DuplicateNodeError`

| | |
| --- | --- |
| **When** | Two nodes are added under the same id via `node` / `agentNode` / `toolNode` / `component` / `humanGate`. Thrown immediately at build time, not at compile. |
| **Handle** | Give each node a unique id. This is a programming error, not a runtime condition — fix the graph definition. |

### `MissingHandlerError`

| | |
| --- | --- |
| **When** | An `action` node is added via `node(id, config)` with no `handler`. Thrown immediately at build time. |
| **Handle** | Provide a handler, or use a node type that doesn't need one (e.g. `humanGate`, `agentNode`, `component`). |

### `GovernanceMiddlewareRejectedError`

| | |
| --- | --- |
| **When** | An `agentNode`'s `middleware` names a **governance** kind (`redact` / `approvalGate` / `fsPolicy`). Those are engine-injected and sealed — only efficiency kinds (`compress` / `terse` / `contextBudget` / `reflection`) are user-supplied. Thrown at build time. |
| **Handle** | Remove the governance kind. Governance is always on — you cannot add, remove, or replace it (see [middleware & profiles](/docs/advanced-agents/middleware-and-profiles#governance-kinds-are-rejected)). This is the *governed-by-construction* guarantee: an ungoverned stack is unrepresentable. |

## TypeScript: engine errors and interrupts

These come from `@ailu-ai/graph-runtime` and `@ailu-ai/agents-core`. They surface during
execution, and on the Rust path most are reported as a `run_failed` / `node_failed`
[`RunEvent`](/docs/reference/events-and-streams) rather than thrown into your `await`.

### `DynamicInterrupt`

`@ailu-ai/graph-runtime` (re-exported from the SDK). **Not an error condition** — it is the
mechanism a node uses to suspend the run cleanly. Carries `reason: string` and an optional
`patch: Record<string, unknown>` persisted into state.

| | |
| --- | --- |
| **When** | An agent node with `suspendForApproval` reaches a gated tool (reason `"agent-approval-required"`); a `human-gate` node; or a tool node whose tool is `requiresApproval`. |
| **Handle** | Don't catch it — let it suspend. The run yields a `run_suspended` event and a `suspended` status; continue with [`resume`](/docs/reference/builder-api#resumerunid) or [`approveAndResume`](/docs/reference/builder-api#approveandresumerunid-options). |

:::warning Tool-node interrupt on Rust is a failure, not a suspension
A `toolNode` whose tool is `requiresApproval` throws a `DynamicInterrupt` that suspends cleanly
on the TS engine, but surfaces as a **node failure** on the Rust engine. Route such graphs with
`AILU_SDK_ENGINE=ts`. (Source: `compiled-graph.ts`.)
:::

### `RecursionLimitError`

`@ailu-ai/graph-runtime`. Message: `Recursion limit exceeded (<limit>)`.

| | |
| --- | --- |
| **When** | A cyclic graph runs past its `recursionLimit` (set via `createGraph({ recursionLimit })`). This is the guard that stops a looping agent from spinning forever — see the [execution contract](/docs/core-concepts/execution-contract). |
| **Handle** | Raise `recursionLimit` if the loop is legitimately long; otherwise fix the cycle's exit condition (a `conditionalEdge` that eventually routes out). |

### `ToolException`

`@ailu-ai/graph-runtime`. Carries `toolId` and `originalError`; its message is the original
error's message (or `"Unknown tool error."`).

| | |
| --- | --- |
| **When** | A tool's `execute` throws while a tool node is running it. |
| **Handle** | Inspect `error.toolId` and `error.originalError`. Make tool handlers defensive (the [integration components](/docs/reference/component-catalog#integration-components-vendor-io) surface I/O failures as data rather than throwing). |

### `StepBudgetExceededError`

`@ailu-ai/agents-core`. Carries `maxSteps` and `currentSteps`; message:
`Step budget exceeded: <current>/<max>`.

| | |
| --- | --- |
| **When** | An agent pattern that enforces a step budget runs past `maxSteps`. |
| **Handle** | Raise the budget, or tighten the agent's stopping condition. |

### `GraphValidationError`

`@ailu-ai/graph-core`. Carries `code: GraphValidationErrorCode` and `path:
GraphValidationPath`. You rarely catch this directly — it arrives inside a
`GraphCompileError.errors` array. The `code` vocabulary:

| `code` | Meaning |
| --- | --- |
| `DUPLICATE_NODE_ID` | Two nodes share an id. |
| `DUPLICATE_EDGE_ID` | Two edges share an id. |
| `MISSING_ENTRY_NODE` | No entry node resolved. |
| `INVALID_EDGE_REFERENCE` | An edge references a node that doesn't exist. |
| `CYCLE_DETECTED` | A cycle the validation model rejects. |
| `INVALID_CONDITION_FORMAT` | A conditional edge's condition is malformed. |

## Python: `ValueError` subclasses

The Python SDK (`pip install ailu`, then `import ailu`) is a thin wrapper over the
same Rust engine. It raises three `ValueError` subclasses (`python/ailu/__init__.py`), so
`except ValueError` catches them all.

### `GraphValidationError(ValueError)`

| | |
| --- | --- |
| **When** | `validate_graph(definition)` cannot serialise/parse the definition at the JSON boundary. |
| **Handle** | Catch it for malformed input. **Note:** a *structurally invalid* graph does **not** raise — `validate_graph` returns the list of validation-error dicts (each with `code`, `message`, `path`) instead. An empty list means structurally sound. |

```python
import ailu

errors = ailu.validate_graph(definition)  # returns a list, does not raise on bad structure
if errors:
    for e in errors:
        print(e["code"], e["message"])
```

Expected result: prints one line per structural problem; nothing if the graph is sound.

### `GraphCompileError(ValueError)`

| | |
| --- | --- |
| **When** | `compile_graph_yaml(yaml)` fails to parse, compile, or validate the DSL YAML. |
| **Handle** | `except ailu.GraphCompileError` and surface the message; the underlying Rust error is the chained cause (`from error`). |

### `RunError(ValueError)`

| | |
| --- | --- |
| **When** | `run_component(...)` / `run_prebuilt(...)` fails — an unknown component kind or agent name, invalid params/input, non-JSON-serialisable params/input, or a handler/runtime failure reported by the Rust engine. |
| **Handle** | `except ailu.RunError`; the message identifies the cause. |

:::note One engine, identical semantics
The Python and TypeScript SDKs share the same Rust validator and DSL compiler, so a graph that
validates (or fails) in one validates (or fails) the same way in the other. The error *shapes*
differ by language idiom (TS typed classes vs Python `ValueError` subclasses); the *causes* are
the same. (Source: the Python SDK module docstring and the shared `crates/py-bindings`.)
:::

## The `Result` discriminated union

`safeCompile()` returns a `Result` instead of throwing — the same ergonomics as Zod's
`safeParse` (`packages/graph-sdk/src/errors.ts`):

```ts
type Result<T, E> = { success: true; data: T } | { success: false; error: E };
```

For `safeCompile`, `T` is `CompiledGraph<TState>` and `E` is `GraphCompileError`:

```ts
const result = createGraph({ name: "x" }).safeCompile();
if (result.success) {
  await result.data.run({});
} else {
  for (const v of result.error.errors) console.error(v.code, v.message);
}
```

Expected result: with no nodes declared, takes the `false` branch and logs `MISSING_ENTRY_NODE`.

Narrow on `result.success` first — TypeScript then refines `result` to the matching arm, so
`result.data` and `result.error` are each only reachable where they exist.

## Error codes

Beyond its message, every Ailu error carries a stable **`code`**, a one-line **`hint`** (the
fix) and a **`docUrl`** pointing back here — so a human or an AI agent can self-correct from the
failure. Switch on `code` (it won't drift with wording); SDK errors also offer `.format()`
(message + hint + docs in one string). The codes:

### AILU_GRAPH_COMPILE
`.compile()` on a graph that failed validation. **Fix:** read the listed validation errors (each has its own code — a dangling edge, missing entry node, unknown channel) and correct them; or use `safeCompile` for a `Result` instead of a throw.

### AILU_DUPLICATE_NODE
Two nodes added under the same id. **Fix:** unique id, or drop the earlier `.node("<id>", …)`.

### AILU_MISSING_HANDLER
An action node with no handler. **Fix:** pass `.node("<id>", async () => ({ … }))`, or use `.agentNode` / a component node.

### AILU_UNKNOWN_NODE
An edge/condition/fan-out references a node that wasn't added. **Fix:** add the node before referencing it.

### AILU_GOVERNANCE_MIDDLEWARE_REJECTED
`middleware[]` named a governance kind (`redact`/`approvalGate`/`fsPolicy`). Governance is engine-injected and sealed (ADR 0025). **Fix:** remove it — only `compress`/`terse`/`contextBudget` are user-supplied.

### AILU_RUST_ENGINE_REQUIRED
The native Rust engine isn't available, or the graph uses a removed TS-only feature; there is no TS fallback (ADR 0016). **Fix:** install `@ailu-ai/napi` (prebuilt — see [Quickstart](/docs/getting-started/quickstart)); or switch to channel-based routing/approvals.

### AILU_NO_SUSPENDED_STATE
`resume`/`approve` with no suspended state for that run id on this `CompiledGraph`. **Fix:** resume on the **same** instance before the process restarts, or rehydrate from a persisted checkpoint.

### AILU_LEGACY_TS_AGENT_HANDLER
A run reached the removed TS-fallback agent handler (needed `llm`). **Fix:** give the agent node a `model` (e.g. `model.openai("gpt-4o")`) so it runs natively on Rust.

### AILU_UNKNOWN_PROVIDER
A model spec named an unknown provider. **Fix:** use a known provider (`openai`, `anthropic`, `gemini`, `mistral`, `ollama`, `openrouter`, `minimax`, `huggingface`, `lmstudio`) or `model.openaiCompatible({ baseURL })`.

### AILU_MISSING_PROVIDER_KEY
A named provider has no API key in the env. **Fix:** set the named variable (e.g. `OPENAI_API_KEY`) or pass `apiKeyEnv`. Keys come from the environment only, never inlined.

### AILU_NO_PROVIDER_IN_ENV
A provider-less model (`model.fast`, `model.invoke()`) found no API key to resolve a provider from. **Fix:** set one of the listed env vars, or name a provider — `model.openai("gpt-4o")`.

## Next

- [Builder API](/docs/reference/builder-api)
- [Events and streams](/docs/reference/events-and-streams)
- [Execution contract](/docs/core-concepts/execution-contract)
