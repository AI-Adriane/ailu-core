---
sidebar_position: 0
title: The governance moat
description: How Ailu's governance primitives compose into one claim — a run that is gated, attested, replayable, and remembered, with proof anyone can verify.
---

# The governance moat

Most agent platforms tell you **what they logged**. Ailu hands you a run that is **gated,
attested, replayable, and remembered** — and a signed artifact a third party re-verifies on the
open-source engine, **without trusting the vendor**. Four primitives compose into that claim. None is
novel alone; the **governed, composed** version is the moat.

## 1. Human gates — no self-approval

A sensitive tool **suspends** the run; a human (a *different* principal) approves; only then does it
execute. The no-self-approval rule is enforced in the engine, not just the UI. Runs are deterministic
and **resumable**, so a gate is a clean checkpoint, not a lost session.

→ [Approval gates](./approval-gates.md) · [The approval decision](./approval-decision.md)

## 2. Attestation — tamper-evidence

Every approval decision is signed (Ed25519) and **hash-chained** to the previous one. Altering any
record breaks the chain. The chain is exportable as an offline-verifiable proof bundle.

→ [Tool approval & attestation](./tool-approval-and-attestation.md)

## 3. Replay as evidence — faithfulness

Because runs are deterministic and the LLM I/O is journaled, a run can be **re-derived** from its
entry state and checked: did the replay reproduce the decisions it was attested for? Two **independent**
guarantees — the chain proves nothing was altered; the replay proves the attested decisions are the
ones the run actually made.

→ [Replay as evidence](./replay-as-evidence.md)

## 4. Governed memory — and it stays honest

Agents **recall** what they learned across runs and **remember** new facts mid-run, through governed
tools sealed to a tenant-scoped namespace. Memory is **updatable without breaking replay**: an update
*supersedes* (a new, immutable version), a forget *tombstones* (audit-preserved) — so a replay still
reads the snapshot the run saw. Every write is attributable.

→ [Recipe: agent memory](../recipes/agent-memory.md)

## Defense in depth

Secrets + PII are redacted before any model call (deterministic floor + a pluggable seam), and
sensitive output channels can be marked no-log — durability is not observability.

→ [PII & secrets redaction](./pii-redaction.md) · [Compliance framework](./compliance-framework.md)

## See it end to end

The [governed refund agent](../recipes/governed-refund-agent.md) walks one run through the whole
chain: a refund gate → human approval → signed attestation → replay verification → an exported,
independently-verifiable certificate.

## Why it's a moat

- **Independently verifiable.** The proof bundle is checked with stock crypto + the **open-source**
  engine — re-run it yourself. A closed, log-after-the-fact platform structurally cannot offer this.
- **Composed, not bolted on.** Gates, attestation, replay, and memory share one deterministic,
  checkpointed runtime — adding any one to a stateless agent framework is an architecture lift.
- **EU-sovereign, open-core.** Run it on your own infrastructure; the engine is open.
