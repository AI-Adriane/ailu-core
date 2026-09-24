---
sidebar_position: 1
title: Sandboxes overview
description: Code / shell execution is never in the OSS engine — it is a planned external, approval-gated seam, env-configured like the PII / LLMLingua / fs-backend seams. Execution always passes an approval gate.
---

# Sandboxes overview

:::caution Planned — external gated seam, not shipped
The sandbox seam is **planned**, not implemented. The seam *pattern* is established (PII,
LLMLingua, fs-backend all follow it); a sandbox service + an `execute` tool are future work.
Nothing on this page runs today. It documents the **deliberate design** so the contract is
fixed before any code lands. See **[ADR 0023](https://github.com/AI-Adriane/ailu-core/blob/main/docs/adr/0023-governed-deep-agent-platform-landscape.md)**.
:::

Code and shell execution let an agent run a program — an interpreter, a build, a script. In
Ailu this capability lives **outside** the engine and **behind an approval gate**, by design.

## Why it is never in the engine

The OSS security rule forbids `eval` / `new Function` / dynamic `import()` / exec of user strings
**anywhere in the engine**. Conditions are named predicates, never eval'd code; the engine has no
path that runs an arbitrary string. Putting an interpreter in the engine would break that
invariant for every deployment.

So code/shell execution is an **external seam**: an env-configured service the engine calls over
a tiny HTTP contract — the same shape as the [PII redaction seam](/docs/governance/pii-redaction),
the LLMLingua compression seam, and the durable `fs` backend. The engine ships the **hook and the
gate**, not the executor. You bring (or buy) the isolated runtime.

## Always approval-gated

This is the load-bearing rule and the reason it is a seam, not a tool you can drop in raw:
**every `execute` passes through an approval gate before it runs.** There is no "fast path" that
skips it.

- The `execute` tool is marked `requiresApproval`; an agent reaching for it **suspends the run**
  the moment execution is requested — the code does **not** run until a human grants it.
- The principal that **requests** execution and the principal that **approves** it must differ —
  an agent never approves its own `execute`. Same no-self-approval rule as every other gate.
- Grant → resume → run → checkpoint → signed audit. The execution is an audited node like any
  other powerful action.

See **[Approval gates](/docs/governance/approval-gates)** for the two seams (`humanGate` and
agent-native `suspendForApproval`) that this builds on. The planned `execute` tool rides the
second seam — it is a sensitive, `requiresApproval` tool, granted with `approveAndResume`.

## Planned configuration

Wiring will follow the established seam convention — point one env var at an isolated execution
service, leave it unset for a no-op (no executor, no `execute` tool offered):

```bash
# PLANNED — not yet read by the engine. Shape matches the existing seams.
export AILU_SANDBOX_URL="https://your-sandbox.internal/execute"
export AILU_SANDBOX_TOKEN="…"   # optional bearer, sent only if set
```

```ts
// PLANNED usage — the execute tool is gated by construction.
import { createGraph } from "@ailu-ai/graph-sdk";

const app = createGraph({ name: "code-agent" })
  .agentNode("worker", {
    llm,
    prompt: { system: "Use the sandbox to run code when needed." },
    tools,                  // includes the gated `execute` tool when the seam is configured
    suspendForApproval: true // run suspends before any execute; nothing runs un-approved
  })
  .compile();

// The agent asks to run code → the run suspends; the sandbox is NOT called.
const suspended = await app.run();
// A human grants the specific action, then the run resumes and the sandbox runs it.
const done = await app.approveAndResume(suspended.runId, { approvedTools: ["execute"] });
```

The exact env var names, the wire contract, and the tool id are **not final** until the seam ADR
is signed off — treat the snippet above as the intended shape, not an API to depend on.

## Where it sits among the seams

| Capability | Placement | Status |
| --- | --- | --- |
| Models / LLM calls | LLM Gateway (one provider seam) | Shipped |
| PII redaction | external HTTP seam (`AILU_PII_REDACTOR_URL`) | Shipped |
| Prompt compression (LLMLingua) | external HTTP seam (`AILU_LLMLINGUA_URL`) | Shipped |
| Virtual filesystem backend | external HTTP seam (`AILU_FS_BACKEND_URL`) + per-path permission DSL → gate | Shipped |
| **Code / shell execution (sandbox)** | **external gated seam — never in-engine; `execute` always gated** | **Planned** |
| Interpreters | same external sandbox seam | Planned |

Adding code/shell to an existing family is a seam + a gated tool, not a change to the engine's
no-exec invariant.

## What the engine ships vs what you bring

| Engine (open source) | You / your control plane |
| --- | --- |
| The no-exec invariant (no `eval`/exec of strings in-engine) | The isolated runtime (container / microVM / WASM / vendor sandbox) |
| The approval gate + no-self-approval + signed audit (shipped) | The execution policy (which commands, limits, network, time) |
| *(Planned)* the `execute` seam hook, env wiring, wire contract, gated tool | *(Planned)* the sandbox service that speaks the contract |

The same bet as the rest of the platform: keep heavy, untrusted execution **out** of the lean OSS
core, and make the version Ailu offers **governed by construction** — execution that cannot
run without a gate.

## See also

- [Approval gates](/docs/governance/approval-gates) — the gate this seam always passes through.
- [PII redaction seam](/docs/governance/pii-redaction) — the reference external seam shape.
- [Models overview](/docs/integrations/models/overview) — the BYOM provider seam.
