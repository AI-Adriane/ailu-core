# ADR 0048 — An approval for named values: the gate opens when an argument is one of them

- Status: **Accepted** — 2026-10-04, with the product's ADR 0111, which names this engine change
  and its release (Mathieu, 2026-10-04: « Je valide tous les design et enchaîne la suite »).
- Date: 2026-10-04
- Deciders: Mathieu (owner)
- Driven by the product repo's ADR 0111 D3: « Exiger une approbation » on a remote agent gates the
  delegations to THAT agent only. The product writes it as a rule on `a2a_delegate` whose condition
  names the agent — `approvalWhen: [{ argument: "agentName", in: ["Nordlys"] }]` — and ADR 0111
  names this as its one engine change, released from this repository before the product uses it.
- Relates to: [0046](./0046-an-approval-above-a-threshold.md) (conditions on a gated tool's
  arguments, decided per call — this ADR adds the test 0046 left out: « conditions on a string »),
  [0025](./0025-unified-agent-middleware-api.md) (the approval gate, intrinsic to the stack),
  [0045](./0045-host-nodes-in-rust-and-sdk-parity.md) (Rust first, both SDKs in the same release).

## Context

Read from the source on 2026-10-04 (release 2.4.0).

- **A condition compares a number.** `ApprovalCondition { argument, above }`
  (`crates/agents-core/src/tools.rs`): the gate of a conditioned tool opens per call when the
  argument is absent, not a JSON number, or above its threshold (`crossings`). ADR 0046 left « comparisons
  other than `above`, conditions on a string » undecided.
- **One tool delegates to every remote agent.** An agent delegates through `a2a_delegate`; its
  `agentName` argument names the registration it calls. A gate by name stops every delegation, to
  every agent; a threshold cannot say « only when the target is Nordlys ». So the product's
  inspector draws « Exiger une approbation » on a remote agent as planned.

## Decision

### D1 — A condition may name values, as data

A condition carries exactly one test: `{ argument, above }` (ADR 0046) or `{ argument, in }`, where
`in` is a non-empty list of non-empty strings. Never an expression, never a pattern.

- Rust: `ApprovalCondition { argument, above: Option<f64>, one_of: Option<Vec<String>> }` — the
  wire names stay `above` and `in`, so a 2.4.0 condition reads unchanged.
- A condition with both tests or neither, an empty `in`, or an empty value is refused when the
  agent is built — never ignored — next to 0046's refusals (an empty argument, a threshold that is
  not finite, a tool that is not approval-gated).

### D2 — The gate decides per call, fail-closed

For a condition `{ argument, in }`, the gate reads the argument from the call's input:

- absent, or not a JSON string → the gate opens (`agentName missing`, `agentName not a string`);
- a string equal — byte for byte: no case folding, no trimming — to one of the values → the gate
  opens, naming it (`agentName = "Nordlys"`);
- another string → this condition is not crossed.

As in 0046, a tool's conditions combine: the gate opens when any one is crossed, and every one
crossed is named. A tool without conditions is gated by its name exactly as before.

### D3 — The grant is that call, and the filing is 0046's

A gate opened by a named value is content-scoped like a threshold's: its key is
`"<name>#<sha256(canonical input)>"`, a signature unlocks that exact call, the request carries the
key, the input and what crossed, and a catalog run files them (0046 D3, D4). Nothing new on the
host side: a host that files 0046's keys files these.

### D4 — Rust first, both SDKs in the same release

The test, the gate and the validation are in the Rust core. TypeScript: a tool definition's
`approvalWhen` accepts `{ argument, in }` (`@ailu-ai/agents-core`, the agent and catalog carriers,
the contracts carrier schema). Python: `approval_when` passes `in` through to the carrier.
Released together as **2.5.0**.

## Consequences

- **Compatibility.** A graph without `in` behaves byte-for-byte as in 2.4.0.
- **Replay.** The values are in the graph; a replay gates exactly where the run did.
- **Names are data.** A list of values is part of the graph or policy that carries it: adding an
  agent to it is a new version of that graph or policy, signed and replayed like any other.
- **Exact match.** The engine compares bytes. A host that wants names compared otherwise
  normalizes them before it writes the list, and its tool before it reads the argument.

## Not decided here

- Nested arguments (`target.name`), `not in`, patterns, numbers or booleans in `in`.
- A condition on a human-gate node (only tools are conditioned, as in 0046).
