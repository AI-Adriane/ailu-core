---
sidebar_position: 5
title: Built for AI agents
description: Ailu is legible to the coding agents that use it — an llms.txt index, per-node JSON Schema, and a one-call run explainer.
---

# Built for AI agents

The developer using Ailu is increasingly an **AI coding agent**. Ailu is built to be legible to
one: it ships a machine-readable index of its own surface, JSON Schema for every node param, and a
one-call explainer that turns a run's state into a next-action sentence. Together these let an agent
discover the API, validate its own graph, and recover from a suspended/failed run without a human.

## `llms.txt` — a machine-readable index

`generateLlmsTxt()` returns the [llms.txt](https://llmstxt.org/)-style index of the SDK surface: the
builder methods, the node kinds, the component catalog, and the doc map — the thing you hand an agent
so it knows what exists before it writes a line.

```ts
import { generateLlmsTxt } from "@ailu/graph-sdk";

await Bun.write("llms.txt", generateLlmsTxt()); // or fs.writeFileSync
```

Serve it at `/llms.txt`, or commit it so an agent reading the repo finds it. It is generated from the
same catalog the runtime uses, so it never drifts from the real API.

:::tip Live on this site
This documentation serves its own index at [pathname:///llms.txt](pathname:///llms.txt) — point your
coding agent there first.
:::

## Per-node JSON Schema

Every component node param is described by JSON Schema, so an agent can validate a graph it is about
to build (and editors can autocomplete it):

```ts
import { componentSchema, componentSchemas, paramTypeToJsonSchema } from "@ailu/graph-sdk";

const all = componentSchemas();        // Record<kind, ComponentSchema> — the whole catalog
const one = all["promptBuilder"];      // { kind, description, params: JSON Schema }
```

`paramTypeToJsonSchema` exposes the same conversion for a single param type — useful when generating
a form or a tool definition from a node's shape.

## `explainRun` — a one-call run explainer

`explainRun(state, events?)` turns a run's `GraphState` (plus an optional event log) into a compact,
**agent-readable** `RunExplanation`: status, current node, a one-line summary, and — crucially — the
**concrete next action** when the run is suspended or failed.

```ts
import { explainRun } from "@ailu/graph-sdk";

const x = explainRun(state, events);
console.log(x.summary);
if (x.suspended) console.log("To resume:", x.suspended.nextAction);
```

| `RunExplanation` field | Meaning |
| --- | --- |
| `runId` / `status` / `currentNode` | Where the run is. |
| `summary` | One line: the situation + the next action. |
| `suspended?` | `{ reason, node, awaitingSignal?, wakeAt?, nextAction }` — why it paused and exactly how to unblock it. |
| `failure?` | `{ node?, error }` — the most recent failure from the event log. |
| `channels` | The declared channel names (engine-internal `__*` channels dropped). |
| `recentEvents?` | The last few lifecycle events (type + node), when an event log is provided. |

The `nextAction` is literal — e.g. *"deliver the `approval` signal with `app.signal(runId, …)`"* — so an
agent can act on it directly. This is the recovery loop that lets an autonomous agent drive a governed
run end-to-end.

## See also

- [Watch a run in the inspector (`ailu dev`)](/docs/recipes/dev-inspector)
- [MCP server — Ailu as a machine API](/docs/building/mcp-server)
- [Typed errors](./errors) — every error carries a `code`, a `hint`, and a `docUrl`.
