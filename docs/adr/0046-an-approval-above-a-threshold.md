# ADR 0046 — An approval above a threshold: the engine decides per call, and the grant is that call

- Status: **Proposed** — 2026-10-03, for the owner (Mathieu).
- Date: 2026-10-03
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0104 D3 (accepted 2026-10-01): « above a threshold, two
  signatures » — a rule says when it applies, `when: { argument: "amount", above: 500 }`; below
  the threshold the call runs without a gate. ADR 0104 names this as its one engine change,
  released from this repository before the product uses it.
- Relates to: [0024](./0024-governed-virtual-filesystem-seam.md) (content-scoped grants, phase 2c),
  [0025](./0025-unified-agent-middleware-api.md) (the approval gate, intrinsic to the stack),
  [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (the engine decides a catalog run's
  approvals; Rust first, both SDKs in the same release).

## Context

Read from the source on 2026-10-03 (release 2.3.0).

- **A gate is decided by the tool's name.** `ToolDefinition.requires_approval`
  (`crates/agents-core/src/tools.rs`) is set from the agent's `approvalToolNames`; the gate in
  `MiddlewareStack::before_tool` (`crates/agents-core/src/middleware.rs`) opens for every call of
  such a tool unless its grant key is in `__approvedTools`. Nothing reads the call's arguments.
- **A grant unlocks the tool for the rest of the run** — the key is the tool's name — except a
  *content-scoped* tool (the guarded fs writes, ADR 0024 phase 2c): its key is
  `"<name>#<sha256(canonical input)>"`, the request carries that key and the input, and the bridge
  validates a granted key's shape on resume (`validate_approved_tools`).
- **A catalog run files only a description.** `ApprovalSubject { description }`
  (`crates/runtime-bridge/src/catalog_approvals.rs`) drops the request's `approvalKey` and
  `input`: a host that stores what the engine filed cannot give a content-scoped key back on
  resume, nor show the signer the arguments.

## Decision

### D1 — A gated tool may carry conditions on its arguments, as data

An agent's gated tool may carry `approvalWhen`: a list of `{ argument, above }`. `argument` names a
top-level field of the call's input; `above` is a finite number. Never an expression, never code.

- Rust: `ToolDefinition.approval_conditions: Vec<ApprovalCondition>`; the agent spec carries
  `approvalWhen: { "<tool>": [{ "argument": "amount", "above": 500 }] }` next to
  `approvalToolNames`, in the TypeScript and the catalog carrier alike.
- A condition on a tool that is not in `approvalToolNames`, an empty `argument`, or an `above`
  that is not a finite number is refused when the agent is built — never ignored.

### D2 — The gate decides per call, fail-closed

For a gated tool **without** conditions nothing changes. For one **with** conditions, the gate
reads each condition's argument from the call's input:

- it is absent, or not a JSON number (`"600"` written as text is not a number) → the gate opens;
- it is a number above `above` for **any** condition → the gate opens;
- otherwise — every argument present, numeric and at or below its threshold — the call runs, as an
  ungated call does.

### D3 — Above the threshold, the grant is that call

A gate opened by a condition is **content-scoped**: its key is `"<name>#<sha256(canonical
input)>"` (the existing `approval_key`), so a signature unlocks that exact call, never the tool
for the rest of the run — another call goes through the conditions again. The request carries the key and
the input, and its subject names what crossed: `tool:refund — amount 600 > 500` (or
`— amount missing`, `— amount not a number`).

### D4 — A catalog run files the key and the input

`ApprovalSubject` gains `approvalKey` and `input` (both optional, omitted when absent): the host
stores them with the request, gives the key back on resume (`approvedTools[].key`, already
validated by the bridge), and shows the signer the arguments. A request filed before this change
has neither, and resumes as before.

### D5 — Rust first, both SDKs in the same release

The condition, the gate and the filing are in the Rust core. TypeScript: a tool definition's
`approvalWhen` (`defineTool`, `agentNode`, the catalog carrier). Python: `approval_when` on a
tool and the carrier. Released together as **2.4.0**.

## Consequences

- **Replay.** The conditions are in the graph; a replay gates exactly where the run did.
- **Compatibility.** A graph without `approvalWhen` behaves byte-for-byte as in 2.3.0; a host that
  ignores `approvalKey` keeps unlocking by name, which a conditioned gate refuses on resume (the
  key is required) — the product reads the key before it authors a condition.
- **Signers.** The product's two signatures and cooldown (ADR 0104 D1, D4) apply to a conditioned
  gate as to any other: they are decided on the approval, not in the engine.

## Not decided here

- Nested arguments (`order.amount`), comparisons other than `above`, conditions on a string.
- A condition on a human-gate node (only tools are conditioned here).
