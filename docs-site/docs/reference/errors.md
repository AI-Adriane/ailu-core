---
title: "Errors"
description: "Every error code the SDK throws, what it means, and how to fix it."
---

# Errors

SDK errors carry a stable `code`, a `hint` that says how to fix the problem, and a `docUrl` that
points to its entry below. `error.format()` prints all three.

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=errors
```

## Building a graph

### AILU_GRAPH_COMPILE

`GraphCompileError`. The graph is invalid: an edge to a node that doesn't exist, a missing entry
node, an unknown channel, and so on. The message lists every problem with its own code. Fix each one, or
use `safeCompile()` to get the problems as a value.

### AILU_DUPLICATE_NODE

`DuplicateNodeError`. Two nodes have the same id. Give each node a unique id.

### AILU_MISSING_HANDLER

`MissingHandlerError`. A `.node()` was added without a handler. Pass one:
`.node("id", async () => ({}))`.

### AILU_UNKNOWN_NODE

`UnknownNodeError`. An edge, a condition or `entry()` names a node that doesn't exist. Add the node
before you reference it.

### AILU_GOVERNANCE_MIDDLEWARE_REJECTED

`GovernanceMiddlewareRejectedError`. An agent's `middleware` list contains a governance kind
(redaction, approval gate, file policy). Those are applied by the engine and can't be set or
removed per agent. Keep only `structuredOutput`, `terse`, `contextBudget`, `compress` and
`reflection`.

## Running a graph

### AILU_RUST_ENGINE_REQUIRED

`RustEngineRequiredError`. The native engine didn't load, so the graph can't run. Check that your
platform is supported ([Install](../install.md#requirements)). This error also appears when a
graph uses a removed option: an `approvalEngine` on `agentNode`, or `AILU_SDK_ENGINE=ts`. See
[Migrating](./migration.md).

### AILU_NO_SUSPENDED_STATE

`ResumeStateNotFoundError`. `resume`, `approveAndResume` or `signal` was called with a run id this
`CompiledGraph` doesn't hold: another instance started it, the process restarted, or the run
already finished. To resume across processes, use the catalog runner
([Long-running runs](../guides/long-running.md)).

### AILU_APPROVER_REQUIRED

`ApproverRequiredError`. `approveAndResume` was called without `resolvedBy`, or with an empty one.
Pass the person who approved, from your authenticated session:
`approveAndResume(runId, { approvedTools, resolvedBy: "alice@example.com" })`.

### AILU_APPROVAL_NOT_GRANTED

`ApprovalNotGrantedError`. `resumeCatalogGraph` was given an `approvalEngine`, and the engine
doesn't authorize the resume: a request the run waits on is still pending, a human gate was
rejected, a tool in `approvedTools` has no request approved by the person the grant names, or the
run was started without the engine. `error.problems` lists each one. Nothing ran; resolve the
requests with `approve(id, approver)` or `reject(...)` and resume again.

### AILU_LEGACY_TS_AGENT_HANDLER

An agent handler from the removed TypeScript engine was called. Build agents with `agentNode` and
run them with `app.run()`.

## Models

### AILU_UNKNOWN_PROVIDER

`UnknownProviderError`. A model or a `provider` option names a provider Ailu doesn't know, such as
`"groq:llama-3"`. Use
one of `openai`, `anthropic`, `google`, `mistral`, `openrouter`, `minimax`, `huggingface`,
`ollama`, `lmstudio`, or `model.openaiCompatible({ baseURL })` for any OpenAI-compatible server.

### AILU_MISSING_PROVIDER_KEY

`MissingProviderKeyError`. `model.<provider>(...).invoke()` found no key. Set the variable the
message names, or `AILU_LLM_MOCK=1` to run offline.

### AILU_NO_PROVIDER_IN_ENV

`NoProviderInEnvError`. A tier-only model (`model.fast`) found no provider key at all. Set one of
the variables in [Models and providers](./models.md), or name a provider.

## Messages from the engine

Some errors come from the engine as plain messages:

| Message starts with | Meaning |
| --- | --- |
| `unknown model provider '...'` | A graph definition names a provider Ailu doesn't know. See `AILU_UNKNOWN_PROVIDER`. |
| `no API key for provider '...'` | An agent's provider has no key. Set the variable it names, or `AILU_LLM_MOCK=1`. |
| `no model provider API key found` | A tier-only agent found no key at all. |
| `Ollama is not enabled` / `LM Studio is not enabled` | Set `AILU_USE_OLLAMA=1` / `AILU_USE_LMSTUDIO=1`. |
| `condition '...' failed` | A conditional-edge predicate threw; the run fails rather than guess a branch. |
