---
sidebar_position: 2
title: Governed virtual filesystem
description: Give an agent file tools — read/write/edit/grep — bounded by a fail-closed per-path policy.
---

# Governed virtual filesystem

A deep agent often needs a scratchpad: somewhere to write notes, read them back, and edit work in
progress across many turns. Ailu gives an agent a **virtual filesystem** — not the host disk, but
a run-scoped store over the versioned artifact store — bounded by a **fail-closed path policy**.

Two pieces:

- **`enableFs`** on an agent node — opt the agent into the eight filesystem tools.
- **`fsPolicy`** on the graph — the per-path permission rules every fs-enabled agent in the run is
  bound by.

```ts
import { createGraph, DefaultLLMGateway } from "@ailu/graph-sdk";

createGraph({ name: "researcher" })
  .fsPolicy([
    { glob: "notes/**", verb: "write" },   // the agent may write under notes/
    { glob: "secret/**", verb: "deny" }    // never touch secret/
  ])
  .agentNode("worker", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Research the topic. Keep notes under notes/." },
    enableFs: true
  })
  .compile();
```

`governed-deep` enables the filesystem for you — see [profiles](./middleware-and-profiles#profiles).

## The filesystem tools

An `enableFs` agent gets eight tools, run-scoped over the artifact store:

| Tool | Verb needed | Purpose |
| --- | --- | --- |
| `read_file` | `read` | Read a file's contents. |
| `ls` | `read` | List a directory. |
| `glob` | `read` | Find files by glob. |
| `grep` | `read` | Search file contents. |
| `write_file` | `write` (or `gate`) | Create / overwrite a file. |
| `edit_file` | `write` (or `gate`) | Apply an edit to a file. |
| `delete_file` | `write` (or `gate`) | Remove a file. |
| `move_file` | `write` (or `gate`) | Rename / move a file. |

The agent is the **principal** recorded on every write, so the audit journal attributes each
mutation to the node that made it.

## The path policy

`fsPolicy` is a list of `{ glob, verb }` rules. A path resolves to the verb of its matching rule;
the verb is one of, in increasing power:

| Verb | Meaning |
| --- | --- |
| `deny` | No access at all. |
| `read` | Read-only. |
| `gate` | Writes are allowed **but each one suspends for human approval** (see below). |
| `write` | Full read/write, no gate. |

Globs use `*` within a path segment and `**` across segments.

### Fail-closed by default

The default — **no policy, or a path that matches no rule** — is **read-only**. An agent can never
write somewhere a rule did not explicitly grant `write` or `gate`. You opt *into* write access, you
never opt out of safety. An empty `fsPolicy` means "read everywhere, write nowhere".

## Gated writes (content-scoped approval)

A `gate` verb routes every write through the human-approval gate. This reuses the same governance
seam as tool approval ([approval gates](/docs/governance/approval-gates)), with one addition: the
grant is **content-scoped**. The approval is pinned to the exact call — the composite key is
`<tool>#<sha256(input)>` — so approving one write to one path with one content does **not** unlock a
different path or different content. A second, different write re-gates. There is no over-grant.

```ts
createGraph({ name: "guarded" })
  .fsPolicy([{ glob: "drafts/**", verb: "gate" }])  // writes under drafts/ need approval
  .agentNode("editor", {
    llm,
    prompt: { system: "Draft under drafts/." },
    enableFs: true,
    suspendForApproval: true   // suspend the run when a gated write is reached
  })
  .compile();
```

The reviewer sees the path **and** the content being written, then grants it with
[`approveAndResume`](/docs/reference/builder-api#approveandresumerunid-options).

## Durable backends

By default the filesystem is backed by the run's artifact store. For a **durable, external** backend
(so files survive across processes and workers), point the engine at an HTTP filesystem service with
the `AILU_FS_BACKEND_URL` env var. That seam **fails closed**: if the configured backend is
unreachable, the operation errors rather than silently falling back to local state.

## Next

- [Deep agents: todos & tasks →](./deep-agents)
- [Approval gates](/docs/governance/approval-gates)
