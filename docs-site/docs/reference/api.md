---
title: "API"
description: "The builder, the compiled graph, the catalog runner and the helpers exported by @ailu-ai/graph-sdk."
---

# API

Everything on this page is exported by `@ailu-ai/graph-sdk`. Types are in the package's
`.d.ts` files; your editor shows them on hover.

## Build a graph: `createGraph`

`createGraph({ name, id?, version?, recursionLimit? })` returns a builder. Every method returns the builder, so calls chain.

| Method | Adds |
| --- | --- |
| `.channel(name, { type, default?, reducer?, noLog? })` | A channel. `reducer`: `"replace"` (default), `"append"`, `"merge"`. `noLog` masks it in events. |
| `.node(id, handler)` | A step: `async (input, state) => ({ ...updates })`. Or `.node(id, { handler, retryPolicy: { maxAttempts, backoffMs }, label })`. |
| `.agentNode(id, config)` | An LLM agent. See below. |
| `.humanGate(id)` | A node that suspends the run until you resume it. |
| `.component(id, components.<kind>({...}))` | A [built-in component](./components.md). |
| `.taskNode(id, { subAgent, objectiveChannel?, reportChannel?, compress? })` | An isolated sub-agent that reads `objective` and writes `report`. |
| `.mapAgents(id, { overChannel, subAgent, joinAt, suspendForApproval? })` | One sub-agent per item of `overChannel`, results in `joinAt`. |
| `.subgraph(id, builder, { inputMapping?, outputMapping? })` | Another builder as one node. Mappings are `destination: source`. |
| `.edge(from, to)` | An edge. |
| `.conditionalEdge(from, to, name, (state) => boolean)` | An edge taken when the named predicate returns `true`. |
| `.errorEdge(from, to)` | Where to go when `from` fails after its retries. |
| `.fanOut(from, [branches], joinAt)` | Run `branches` in parallel after `from`, continue at `joinAt`. |
| `.entry(id)` | The start node (default: the first node added). |
| `.fsPolicy([{ glob, verb }])` | File rules for agents with `enableFs`. `verb`: `read`, `write`, `gate`, `deny`. |
| `.compile()` | Checks the graph and returns a `CompiledGraph`. Throws `GraphCompileError`. `.safeCompile()` returns a result instead. |

### `agentNode` config

| Option | Type | Default |
| --- | --- | --- |
| `model` | `model.*(...)`, or a `"provider:model"` string | Anthropic's default model |
| `prompt` | `{ system: string }` | required |
| `tools` | `InMemoryToolRegistry` | none |
| `suspendForApproval` | `boolean`: stop the run when a gated tool is requested | `false` |
| `outputChannel` | `string` | `"agentResult"` |
| `visibleChannels` | `string[]`: the channels the agent is shown | all |
| `maxIterations` | `number` | engine default |
| `middleware` | `[{ kind: "structuredOutput" \| "terse" \| "contextBudget" \| "compress" \| "reflection", params? }]` | none |
| `profile` | `"fast" \| "frontier-careful" \| "governed-deep"` | none |
| `todosChannel` | `string`: where the `writeTodos` plan is saved | none |
| `enableFs` | `boolean`: file tools, under `.fsPolicy` | `false` |
| `memory` | `{ namespace, topK?, recall? }` | none |
| `skills` | `{ namespace, required?, advisoryK? }` (catalog runner only) | none |
| `inputBlocksChannel` | `string`: a channel of images, audio or files for the model | none |

## Run it: `CompiledGraph`

| Member | Does |
| --- | --- |
| `run(data?, { runId? })` | Runs until the end or a suspension. Resolves with the state: `{ runId, status, currentNodeId, channels }`. |
| `resume(runId)` | Continues a suspended run of this instance. |
| `approveAndResume(runId, { approvedTools, resolvedBy })` | Grants gated tools, then continues. `resolvedBy`, the approver, is required. |
| `signal(runId, name, payload?)` | Delivers a signal to a run waiting on `waitForSignal(name)`. |
| `stream(data, mode, { runId? })` | Runs and yields events. `mode`: `"messages"`, `"updates"`, `"values"`, `"debug"`. |
| `onEvent(handler)` | Subscribes to run events. Returns an unsubscribe function. |
| `explain(runId)` | What a suspended run waits for, and how to continue it. |
| `definition` | The graph as a plain `GraphDefinition` (JSON). |

`status` is `"running"`, `"suspended"`, `"completed"`, `"failed"` or `"cancelled"`.

## Run from saved state: the catalog runner

| Function | Does |
| --- | --- |
| `runCatalogGraph(definition, options?)` | Runs a `GraphDefinition`. Resolves with `{ state, status, pendingApprovals?, replayJournal?, entryState? }`. |
| `resumeCatalogGraph(definition, state, options?)` | Continues a run from a saved state. With `approvalEngine`, first checks that the engine approved what the run waits on. |
| `replayCatalogGraph(definition, entryState, id, replayJournal)` | Re-runs a recorded run without calling a model. |

Options: `initialData`, `runId`, `approvalEngine`, `tools` (`[{ name, execute }]`), `approvedTools`
(resume only: `[{ name, requestedBy, resolvedBy }]`), `signal` (an `AbortSignal`), `onEvent`,
`providerKeys`, `fsPolicy`, `skills`, `subgraphs`, `streamTokens`.

Only agents, components, human gates, subgraphs and `mapAgents` run on this path: your own
`.node()` functions and conditional-edge functions do not.

## Models

`model` and `model.invoke()`: see [Models and providers](./models.md) and
[Agents and models](../guides/agents.md#call-a-model-directly).

## Helpers

| Export | Does |
| --- | --- |
| `finalAnswer(result)` | The answer text of an agent result. |
| `InMemoryToolRegistry` | Holds tools: `register(definition, handler)`. |
| `writeTodosTool` | The built-in planning tool: `register(writeTodosTool.definition, writeTodosTool.handler)`. |
| `waitForSignal(name, { wakeAt? })`, `sleepUntil(date)` | Node return values that suspend the run. |
| `readSignal(state, name)`, `readSuspendMeta(state)` | Read a delivered signal, or why a run is suspended. |
| `council({ members, reviewers?, chair, humanGate? })` | A council graph, for `runCatalogGraph`. |
| `components` | The [built-in components](./components.md). |
| `semanticRetriever`, `createEmbeddings`, `createVectorStore` | Retrieval by embeddings. See [RAG](../guides/rag.md). |
| `InMemoryApprovalEngine`, `Ed25519Attestor`, `verifyChain`, `verifyReplayDecisions` | Approval records and evidence. See [Governance](../guides/governance.md). |
| `exportTracesToOtlp`, `computeCost`, `serveInspector`, `explainRun` | Observability. See [Observability](../guides/observability.md). |
| `compileGraphFile(yaml, fileName)`, `validateGraph(definition)` | YAML graphs. See [YAML and the CLI](../guides/yaml-and-cli.md). |
| `rustEngineAvailable()` | Whether the native engine loaded. |
| `componentCatalog`, `componentSchemas()`, `generateLlmsTxt()` | Machine-readable descriptions of the SDK. |
