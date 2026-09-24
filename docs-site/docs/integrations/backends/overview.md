---
sidebar_position: 1
title: Filesystem backends
description: The pluggable storage seam behind the governed virtual filesystem — the default artifact-store backend, a durable external HTTP backend, and the per-path policy.
---

# Filesystem backends

The [governed virtual filesystem](/docs/advanced-agents/governed-filesystem) gives an agent eight
file tools — `read` / `write` / `edit` / `delete` / `rename` / `ls` / `glob` / `grep`. Those tools
do not touch storage directly; they call a **backend seam** (`FilesystemBackend`). A backend is
**run-scoped** — one instance is bound to a single run — and **policy-agnostic**: permission
enforcement is the [path policy](#the-path-policy)'s job, not the backend's.

Two backends ship; one disables the filesystem entirely:

| Backend | Storage | Durability | When |
| --- | --- | --- | --- |
| Artifact store (default) | The run's versioned artifact store | Per the artifact store | Default. Versioned + attributable for free. |
| HTTP (external) | An external service you run | Survives across processes / workers | Set `AILU_FS_BACKEND_URL`. Fail-closed. |
| Noop | none | n/a | Deployments that explicitly disable the fs. |

You select a backend by **environment**, not in graph code — the same graph runs against any of
them. What you do write in the SDK is the policy and the per-agent opt-in:

```ts
import { createGraph, DefaultLLMGateway } from "@ailu-ai/graph-sdk";

createGraph({ name: "researcher" })
  .fsPolicy([
    { glob: "notes/**", verb: "write" }, // writable scratch space
    { glob: "secret/**", verb: "deny" }  // invisible, fail-closed
  ])
  .agentNode("worker", {
    llm: new DefaultLLMGateway(),
    prompt: { system: "Research the topic. Keep notes under notes/." },
    enableFs: true
  })
  .compile();
```

## The default: artifact-store backend

The default backend maps the virtual filesystem onto the existing **versioned artifact store** —
no new store method, no schema change. The mapping:

| Filesystem op | Artifact-store effect |
| --- | --- |
| `write` | A **new version** of the artifact named by the path. |
| `edit` | Line patches applied to the latest version, written as a new version. The pre-edit version is preserved. |
| `delete` | A **tombstone** — a new empty version with `metadata.deleted = true`. History stays queryable; the file then reads as not-found. |
| `rename` | Copy the latest content to the new name, then tombstone the source. |
| `read` (with a `version`) | Any prior version is still readable — the audit trail is intact. |

Two consequences:

- **Every mutation is versioned and attributable for free.** The acting node id and `principal`
  ride into `Artifact.metadata`, so the audit journal attributes each write to the node that made
  it. A `delete` never destroys history.
- **Directories are synthetic.** The artifact keyspace is flat (path = artifact name). `ls` derives
  directory entries from path prefixes; any prefix under which a live file lives appears as a
  directory. There are no real directory objects to create or remove.

`glob` / `grep` operate over the **live latest set** — the newest non-tombstoned version per name —
so deleted files vanish from listings and searches.

## The durable backend: external HTTP

Point the engine at an external filesystem service to make fs content **durable across processes
and workers** (e.g. so files outlive a suspend/resume that crosses the napi boundary). Configure it
by environment:

| Env var | Required | Meaning |
| --- | --- | --- |
| `AILU_FS_BACKEND_URL` | yes (to enable) | The service endpoint. Each op `POST`s a `{ op, runId, ... }` JSON body here. When unset, the engine falls back to the default backend. |
| `AILU_FS_BACKEND_TOKEN` | no | Bearer token sent as `Authorization: Bearer <token>` on every request. |

```ts
// .env — selects the durable backend for the whole deployment. No code change.
// AILU_FS_BACKEND_URL=https://fs.internal.example.com/op
// AILU_FS_BACKEND_TOKEN=…
```

The service receives one `POST` per operation. The request body carries the op name, the `runId`,
and the op arguments (camelCase: `path`, `content`, `mediaType`, `patches`, `prefix`, `pattern`,
`paths`, plus `principal` / `nodeId` on mutating ops). A `200` response is **either** the op's
result **or** an `{ "error": <FsError> }` envelope — a semantic error (`notFound`,
`permissionDenied`, …) the service reports, surfaced to the agent verbatim.

### Fail-closed

The HTTP backend is **fail-closed**. A transport failure, a non-2xx status, or an unparseable body
becomes a hard error (`ServiceUnavailable` / `Backend`) — never a silent pass-through and never a
fallback to local state. A missing or unconfirmed fs op is a semantic error the agent must reason
about, not a no-op. (Contrast the prompt-compression seam, which fails *open*.)

> The external service that holds the filesystem is **your** component — Ailu defines the wire
> contract (the `POST` body and the `FsError` envelope) but does not ship the server.

## The Noop backend

For deployments that want the agent shape without any filesystem, the Noop backend answers every
op with `NotSupported`. The fs tools then surface that as a tool error. This is distinct from a
fail-closed *policy* (which still lets an agent read where granted) — Noop turns the storage off
entirely.

## The path policy

Backends are policy-agnostic; the **path policy** decides what an agent may do to each path. It is
a pure, DB-free resolver: a normalized path in, a permission verb out.

```ts
.fsPolicy([
  { glob: "**",                verb: "read"  }, // read everywhere (this is also the default)
  { glob: "scratch/**",        verb: "write" }, // writable scratch
  { glob: "drafts/**",         verb: "gate"  }, // writes need human approval
  { glob: "scratch/secret.txt", verb: "deny" }  // a hole in the writable space
])
```

### Verbs

| Verb | Capability |
| --- | --- |
| `deny` | Invisible — no read, no list, no write. Existence is not disclosed (a `deny` path reports as *not-found*, never *permission-denied*). |
| `read` | `read` / `ls` / `glob` / `grep` only. |
| `gate` | Read **plus** write/edit/delete/rename, but each write routes through the [approval gate](/docs/advanced-agents/governed-filesystem#gated-writes-content-scoped-approval). |
| `write` | Read plus ungated write/edit/delete/rename. |

### Resolution

- **Most-specific glob wins.** Specificity = the count of literal (non-wildcard, non-separator)
  characters in the glob, so `scratch/secret.txt` beats `scratch/**` beats `**`.
- **On a specificity tie, the most restrictive verb wins** (`deny` > `read` > `gate` > `write`) —
  fail-closed tie-break.
- **No matching rule → `read`.** An empty policy (or no policy at all) means *read everywhere,
  write nowhere*. You opt **into** write access; you never opt out of safety.

Globs use `*` within a single path segment and `**` across segments (`scratch/*.md` does not match
`scratch/sub/a.md`; `scratch/**` does).

### External seam

The runtime ships a `StaticPathPolicy` (a compiled rule table) and an `fsPolicy` builder over it.
The `PathPolicy` interface itself is an **external seam**: the control plane compiles its own
owner-only rule table into a policy handed to the engine. The engine never reads policy from a
database — it receives an already-resolved, pure resolver.

## Path normalization

Independent of policy and backend, every agent-supplied path is **normalized first**, fail-closed.
Normalization rejects:

- `..` parent traversal
- absolute paths (a leading `/`)
- backslash separators
- null bytes
- empty paths

and collapses `.` and empty segments. The result is a canonical forward-slash name that never
contains `..` and never starts with `/`. This is the barrier (with the store-controlled `run_id`
prefix as defense-in-depth) against escaping the run's keyspace.

## Errors

All backends and the policy speak one error type, returned to the agent loop as a tool-error
string and serializable on the HTTP wire:

| Error | Meaning |
| --- | --- |
| `notFound` | No file at the path (also the response for a `deny` path — deny never discloses existence). |
| `permissionDenied` | A non-deny denial (e.g. a write to a `read` path). |
| `invalidPath` | Failed normalization (traversal, absolute, null byte, …). |
| `invalidEdit` | An out-of-range or inconsistent edit line range. |
| `notSupported` | The backend does not support the op (the Noop backend). |
| `serviceUnavailable` | An external backend was unreachable (HTTP backend, fail-closed). |
| `backend` | A lower-level backend failure. |

## Next

- [Governed virtual filesystem](/docs/advanced-agents/governed-filesystem) — the agent-facing tools
  and the gated-write flow.
- [Middleware & profiles](/docs/advanced-agents/middleware-and-profiles) — `governed-deep` enables
  the filesystem in one word.
