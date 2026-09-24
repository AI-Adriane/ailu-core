---
sidebar_position: 1
title: The governance model
description: Attestation, provenance, separation of duties, and audit — built into the graph.
---

# The governance model

Governance is the reason Ailu exists. Most agent frameworks let you bolt on an "approval
step" as application code. Ailu makes governance a **property of the runtime**, so the
guarantees hold no matter what the application does.

## The three principles

### 1. Separation of duties — an agent never approves its own output

The principal that **requests** a sensitive action and the principal that **approves** it must
be different. An agent cannot approve its own tool call; a user cannot rubber-stamp their own
request through the same identity. This is enforced — not advised — and a violation is rejected,
not logged-and-allowed: the Rust engine guards its resolve entry points, and a control plane on
top (Ailu Studio, or one you build) rejects the request before it reaches the engine (e.g.
`409`). See [no-self-approval](./approval-gates#no-self-approval).

### 2. Provenance — every decision is attributed and attested

An approval records **who** decided, **what** they decided, and **when** — bound to the
authenticated principal, not a free-text field. Decisions are **attested** with an Ed25519
signature, so the audit record is tamper-evident: you can verify after the fact that a decision
was made by the holder of a specific key and has not been altered.

### 3. Audit — the event journal is the record

Because the runtime [emits an event for every transition](/docs/core-concepts/execution-contract),
the journal *is* the audit trail. `run_suspended`, the pending approval, the resolution, and
`run_resumed` are all there, in order, replayable. There is no separate, hand-maintained log
to fall out of sync.

## Defense in depth

The same rule is enforced at **two independent layers**, so neither a control-plane bug nor a
direct-engine call can bypass it:

- **Engine (Rust)** — the engine independently guards approval and resume entry points
  (`ensure_can_resolve`), so even a caller that reaches the engine directly cannot resolve an
  approval as the requesting agent. This guard ships in the open engine.
- **Control plane** — a control plane you build on the SDK, or **Ailu Studio** (the managed
  governance platform), binds the resolver to an authenticated principal and rejects
  self-approval before anything reaches the engine.

If one layer is misconfigured, the other still holds the line. The engine's guard holds even
with no control plane in front of it.

## What flows through a governed run

```text
agent wants to call `refund`  ──▶  approval gate  ──▶  run suspends (run_suspended)
                                                          │
                              human (≠ agent) approves ───┤  decision attested (Ed25519)
                                                          ▼
                                                 resume  ──▶  `refund` executes  ──▶  run_completed
```

Every arrow is an event; every decision is attributed; the requester and approver are
different principals.

## Where to go next

- [Approval gates](./approval-gates) — the `humanGate` and `suspendForApproval` mechanics.
- [Tool approval and attestation](./tool-approval-and-attestation) — gating specific tool calls.
- [Observable runs](./observable-runs) — turning the event journal into a live view.
