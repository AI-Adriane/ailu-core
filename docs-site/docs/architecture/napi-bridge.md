---
sidebar_position: 2
title: The native bridge
description: How the Rust engine is exposed to TypeScript — the napi addon, the three callbacks, and the wire contract.
---

# The native bridge

The Rust engine is exposed to TypeScript through a **native addon** (napi-rs). The SDK loads it
in a `try/catch`; if it genuinely cannot run a graph, the SDK **throws `RustEngineRequiredError`** —
there is **no TypeScript execution fallback** (ADR 0016), so a run never silently degrades. In
Python the equivalent bridge is a pyo3 extension shipped inside the wheel.

## Loading the addon

Resolution order (first found wins), in `engine/crates/bindings/index.js`:

1. `./ailu_napi.node` — local dev build (`scripts/build-napi.sh` / `pnpm napi:build`).
2. `./ailu_napi.<triple>.node` — local per-platform build.
3. `@ailu-ai/napi-<triple>` — prebuilt per-platform package.

Targets covered: **darwin** (arm64/x64), **linux glibc** (x64/arm64), **win32 x64**. On
uncovered targets (linux musl/Alpine, win32 arm64, others) the prebuilt addon is absent — use a
glibc base image (`node:20-slim`) or build it from source with a Rust toolchain. With no addon,
`usesRustEngine` is `false` and running a graph throws `RustEngineRequiredError` (no silent TS path).

## The napi surface

**Synchronous** functions:

- `engineVersion(): string` — the bound Rust engine version.
- `validateGraphJson(definitionJson): string` — a JSON array of validation errors (`[]` if valid).
- `compileGraphYamlJson(yaml): string` — compile graph DSL YAML to a `GraphDefinition` (JSON).

**Asynchronous** functions (return `Promise<string>` = `RunOutcome` JSON):

- `engineRun(specJson, onNode, onCondition, onEvent)` — start a fresh run.
- `engineResume(specJson, onNode, onCondition, onEvent)` — resume from serialized `specJson.state`.
- `engineApproveAndResume(specJson, onNode, onCondition, onEvent)` — grant
  `specJson.approvedTools` (written to the `__approvedTools` channel) and resume.

## The three JS callbacks (seams)

On the Rust side these are `ThreadsafeFunction`s the engine awaits, allowing async round-trips
across the language boundary **without blocking the main JS thread** (`bridge.rs`):

| Callback | Returns | Behaviour |
| --- | --- | --- |
| `onNode(payloadJson)` | `string \| Promise<string>` | A JS node handler (`kind:"node"` → channel-update JSON) or a JS tool `execute` (`kind:"tool"` → result JSON). **Awaited** by Rust. |
| `onCondition(payloadJson)` | `boolean \| string \| Promise<…>` | A named predicate (`{ name, state }`) → boolean. **Awaited** by Rust. |
| `onEvent(payloadJson)` | `void` | Lifecycle-event sink (serialized `RunEvent`). **Fire-and-forget**, never awaited. |

## The `EngineSpec` wire contract

The SDK sends an `EngineSpec` as **camelCase JSON** that must match the
`@ailu-ai/graph-core` types exactly:

```text
EngineSpec {
  graph,            // GraphDefinition
  runId?,
  initialData,      // map<string, value>
  state?,           // serialized GraphState (required by resume/approve)
  approvedTools[],  // written to __approvedTools (approve path)
  agents,           // map<nodeId, AgentSpec>
  componentNodes,   // map<nodeId, ComponentNodeSpec> — native Rust components
  jsNodeIds[],      // nodes whose handler is a JS closure
  jsToolNames[]     // tools whose execute is a JS closure
}
```

`AgentSpec` (per agent node): `{ provider, model?, tier?, system?, toolNames[], maxIterations?,
suspendForApproval, approvalToolNames[], outputChannel? }`. The `tier` is resolved on the Rust
side against env-available providers; an explicit `model` always wins.

`ComponentNodeSpec` (per component node): `{ kind, params }` — executed by a native Rust handler,
**not** routed to the JS seam, even if the id also appears in `jsNodeIds`.

The bridge deserializes the `EngineSpec`, builds the Rust `GraphRuntime`, wires the TSFNs as
seams, drives `start` / `resume` / `approve`, then re-serializes a `RunOutcome`
(`{ state, status, pendingApprovals }`).

:::warning Checkpoint round-trip
The camelCase serialization must match exactly on both sides. A mismatch breaks resume-from-checkpoint.
:::

## Reserved channels (approval path)

- `__approvedTools` — tool names a human approved, written before resume.
- `__approvalIds` — per-tool approval-request ids (the `ApprovalEngine` path, TS side).

An agent never approves its own tools — see [tool approval and attestation](/docs/governance/tool-approval-and-attestation).

## See also

- [Overview](./overview)
- [Runtime and engine](/docs/core-concepts/runtime-and-engine)
