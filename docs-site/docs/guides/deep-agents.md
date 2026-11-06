---
title: "Deep agents"
description: "Agents that plan with a todo list, work in a governed virtual filesystem, delegate to sub-agents and use governed memory."
---

# Deep agents

A deep agent works on a long task on its own: it writes a plan, reads and writes files, hands
parts of the work to sub-agents, and keeps going for many turns. Ailu gives it these abilities
with the same governance as any agent: file access follows a policy, and sensitive writes wait
for a human.

```ts file=packages/graph-sdk/examples/docs/deep-agent.ts region=example
```

## Plan with todos

`writeTodosTool` is a built-in tool: the agent calls it with its whole plan each time it changes
(`{ text, status }` items, status `pending`, `in_progress` or `completed`). With `todosChannel`,
the latest plan is saved in that channel after every turn, so you can show progress, and a
resumed run keeps it.

## Work with files

`enableFs: true` gives the agent file tools: `read_file`, `write_file`, `edit_file`,
`delete_file`, `move_file`, `ls`, `glob` and `grep`. The files live in a virtual filesystem that
belongs to the run: the agent can't reach your disk.

`.fsPolicy([...])` sets what the agent may do, path by path. The most specific rule wins, and
paths no rule matches are read-only.

| Verb | Effect |
| --- | --- |
| `read` | Read only. |
| `write` | Read and write. |
| `gate` | Writes stop the run for a human approval, pinned to that exact path and content. |
| `deny` | No access at all. |

To keep the files outside the process, point the engine at a storage service with
`AILU_FS_BACKEND_URL` (and `AILU_FS_BACKEND_TOKEN`). If that service is unreachable, file
operations fail rather than silently falling back.

## Delegate

Give a deep agent sub-agents with [`taskNode`](./multi-agent.md#delegate-to-a-sub-agent)
(one isolated helper that reports back) or [`mapAgents`](./multi-agent.md#one-agent-per-item)
(one helper per item).

## Presets

`profile` sets sensible defaults in one word. Explicit options always win.

| Profile | Model tier | Behaviour |
| --- | --- | --- |
| `"fast"` | fast | Short answers, tight context budget. For high-volume, low-stakes steps. |
| `"frontier-careful"` | frontier | Roomy context, self-review, stops for approval. For high-stakes reasoning. |
| `"governed-deep"` | balanced | Short answers, 12k context budget, self-review, stops for approval, files enabled. The one-line deep agent. |

## Memory across runs

`memory: { namespace: "customer-42" }` enables memory recall and writes within a runtime build.
Ordinary agents, nested graphs and map items in that build share one in-process store. The
namespace selects a logical partition inside that store; it does not authenticate a tenant
or grant access to another build's memory. Recall uses a simple built-in embedder, not a
semantic model.

:::caution Breaking change in the next release
The former process-global default is removed. Every separate start, resume, approve, signal
or replay call receives fresh in-process memory by default, even when the graph, node, run
and namespace values are identical. A checkpoint does not preserve the store, so a resumed
suspended run does not retain its earlier in-process memory.
:::

The JS, Python and C entry points currently have **no reusable memory-scope option**. Setting
`memory.namespace` is not an opt-in to cross-run persistence. SDK and Python users who need
memory across calls must integrate a trusted host/control plane with durable storage:

- Before each entry, authenticate the owner, authorize the requested memory, and seed its
  recalled texts into the host-owned `__memoryRecall` state channel for middleware recall.
- Persist explicit `rememberMemory` write intents from `__memoryWrites` for ordinary agent
  nodes, or from each joined map-agent result's `memoryWrites`, under the same authorized
  owner. This does not automatically persist the middleware's reasoning trace.
- Supply only authorized facts to each later entry, including resume. These channels are
  integration seams, not caller-supplied authorization; the SDK does not provide the durable
  store or tenant checks automatically. The `recallMemory` tool queries the current
  in-process store; durable preloaded texts are injected by middleware, not loaded into it.

Rust embedders can instead retain an `ailu_runtime_bridge::MemoryScope::default()` and return
`Some(scope.clone())` from `HostCallbacks::memory_scope`. Allocate a distinct scope per
independently authenticated tenant and authorized immutable graph identity. Reusing a clone
explicitly grants access to that entire store, including on resume or replay; never select
it from graph metadata, input channels, run IDs or client-supplied checkpoint state.

## Skills

Skills are reusable playbooks (instructions, sometimes tools) an agent loads when relevant. You
pass the skill records per run, on the catalog runner:

```ts file=packages/graph-sdk/examples/docs/fragments.ts region=skills
```

and set `skills: { namespace, required: ["refund-policy@1"] }` on the agent. Skills are not
loaded by `app.run()` yet.

## Next

- A complete deep agent: [Deep agent example](../examples/deep-agent.md).
