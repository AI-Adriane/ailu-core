---
sidebar_position: 3
title: Tools and tool nodes
description: Register tools an agent can call, execute them in the agent or a dedicated node, and use pure components.
---

# Tools and tool nodes

There are two ways to execute tool calls: **inside the agent node** (via its `tools` registry)
and in a **dedicated `toolNode`**. And for compute steps that don't need a model at all, there
are pure **components**.

## Registering tools

Tools live in an `InMemoryToolRegistry`. Each has a definition plus a handler:

```ts
import { InMemoryToolRegistry, type ToolId } from "@ailu/graph-sdk";

const tools = new InMemoryToolRegistry();
const passthrough = { parse: (value: unknown) => value };

tools.register(
  {
    id: "search_documents" as ToolId,
    name: "search_documents",
    description: "Keyword search over the corpus. Returns the top documents with scores.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: [],
    jsonSchema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] }
  },
  async (input: unknown) => {
    const { query } = input as { query: string };
    return { hits: [{ id: "checkpointing", score: 3 }] };
  }
);
```

Definition fields: `id` / `name` / `description` (identity + what the agent sees), `inputSchema`
/ `outputSchema` (parsers with `.parse(value)`), `jsonSchema` (the schema the LLM is shown),
`permissions` (declared scopes), and `requiresApproval` — when `true`, the tool is **gated**
behind human approval (see [approval gates](/docs/governance/approval-gates)).

## An agent that calls tools

Hand the registry to `agentNode` via `tools`. The ReAct loop emits tool calls, the node executes
them, feeds results back, and iterates up to `maxIterations`. Offline, script the LLM's tool-use
turns with a `responses` array (one element per turn):

```ts
const app = createGraph({ name: "tool-using-agent" })
  .agentNode("assistant", {
    llm: scripted([
      toolTurn("search_documents", { query: "resume after crash" }),  // turn 1: call the tool
      finalTurn("FINAL: Ailu resumes from the latest checkpoint.")  // turn 2: final answer
    ]),
    prompt: { system: "Use the search tool, then answer." },
    tools,
    maxIterations: 5
  })
  .compile();
```

:::note Mock-sequencing rule
The scripted gateway is **stateful** across turns (and across suspend/resume). Each agent turn
consumes the next scripted response. Order them: tool-use turn(s) first, then the `FINAL:` turn.
:::

## A dedicated tool node

When you want a separate node that executes the tool calls emitted by the last AI message — to
run them in parallel, or to make tool execution explicit in the graph — use `toolNode`:

```ts
createGraph({ name: "agent-then-tools" })
  .messagesChannel()
  .agentNode("plan", { llm: scripted([/* … */]), prompt: { system: "Plan tool calls." }, tools })
  .toolNode("run-tools", { tools, parallel: true })
  .edge("plan", "run-tools")
  .compile();
```

`.toolNode(id, { tools, parallel? })` executes the tool calls from the last AI message in
`messages`. A tool flagged `requiresApproval` **suspends the run** (via a dynamic interrupt)
instead of executing.

## Components: pure compute, no LLM

Not every step needs an agent. Ailu ships a catalog of **pure, deterministic components** —
addressed by a `kind` and `params`, run natively on the Rust engine. Add one
with `.component(id, descriptor)` from a `components.*` factory:

```ts
import { createGraph, components } from "@ailu/graph-sdk";

const app = createGraph({ name: "prep" })
  .channel("name", { type: "string", default: "" })
  .channel("prompt", { type: "string", default: "" })
  .component("build", components.promptBuilder({ template: "Hi {{name}}!", into: "prompt" }))
  .compile();

const out = await app.run({ name: "Ada" });
console.log(out.channels.prompt); // "Hi Ada!"
```

A few you'll reach for often:

| Component | Factory | Key params |
| --- | --- | --- |
| Prompt builder | `components.promptBuilder({ template, into })` | render `{{var}}` into a channel |
| Text cleaner | `components.textCleaner({ from, into, stripHtml?, lowercase?, … })` | normalise text |
| Document splitter | `components.documentSplitter({ from, into, by, size, overlap? })` | chunk text |
| Retriever | `components.retriever({ query, into, k?, docs })` | mock-embedding top-`k` |
| Reranker | `components.reranker({ from, into, query? })` | reorder a results array |

The full set (`router`, `conditionalRouter`, `jsonValidator`, `outputParser`, `csvParser`,
`htmlToText`, `regexExtractor`, `bm25Retriever`, `deduplicator`, and more) is enumerated by
`list_components()` and lives in `engine/packages/graph-sdk/src/components.ts`, where each
factory's params are typed. See the [component catalog](/docs/building/components-reference).

:::tip Router safety
`router` and `conditionalRouter` are **pure predicates**, not `eval`'d code — the same safety
guarantee as conditional edges.
:::

### Vendor-I/O components

Two components — `httpFetch` and `webSearch` — are **integrations**, not pure Rust components.
They return a plain node handler (a closure over an injected I/O impl) and are added with
`.node(...)`, not `.component(...)`. Inject a fake impl to keep tests offline.

## Next

[Human approval gates →](/docs/governance/approval-gates)
