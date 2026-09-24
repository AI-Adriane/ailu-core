---
sidebar_position: 3
title: Tool approval and attestation
description: The reserved channels that carry approvals, and how decisions are attested.
---

# Tool approval and attestation

This page is the wire-level companion to [approval gates](./approval-gates): how an approval
actually reaches the engine, and how the decision is made tamper-evident.

## Reserved channels

Two channel names are **reserved** by the engine to carry approval state across a
suspend/resume boundary. They are signalled early so the engine can route them without
guessing:

| Reserved channel | Carries |
| --- | --- |
| `__approvedTools` (`APPROVED_TOOLS_CHANNEL`) | The tool **names** a human granted for this run. |
| `__approvalIds` (`APPROVAL_IDS_CHANNEL`) | The ids of the approval records that authorised them. |

You don't write these by hand. `approveAndResume(runId, { approvedTools })` populates them:

- On the **Rust** path, the engine writes the approved tool names into `__approvedTools` before
  resuming.
- On the **TypeScript** path, the channel is updated directly.

Either way, when the agent re-runs it sees the approved tools in the reserved channel and
executes exactly those — no re-gating, no self-grant.

## The approval record

Each gated request produces an **approval record** with a stable id. Its shape (as surfaced by
a control plane — **Ailu Studio**, or one you build on the SDK):

```json
{
  "id": "<approvalId>",
  "requestedBy": "assistant",     // the agent — the requester
  "subject": "tool:refund",       // what is being approved
  "status": "pending",            // → "approved" | "rejected"
  "resolvedBy": null              // set to the authenticated approver on resolution
}
```

A control plane (Ailu Studio, or one you build) binds `resolvedBy` to the **authenticated
principal** that resolves the request — never a free-text field, and never the requester (that
is the [no-self-approval](./approval-gates#no-self-approval) rule, which the engine also guards
independently at its resolve entry points).

## Attestation

When a decision is recorded, it is **attested with an Ed25519 signature**. The signature binds
the decision (the approval id, the subject, the resolver, the outcome) to the holder of a
specific signing key. After the fact you can verify that:

- the decision was made by the holder of that key, and
- the recorded decision has not been altered.

### A durable, hash-chained record

A single signature proves one decision wasn't altered. It does **not** stop someone from
*dropping* an awkward decision, *reordering* two, or *inserting* one after the fact. So
attestations for a run are **hash-chained**: each record carries the hash of the previous
decision on that run (`prevHash`), and the first link's `prevHash` is `null`. To tamper with
any decision — or to remove or reorder one — you would have to re-sign every record after it,
which you cannot do without the signing key. The **sequence** is now tamper-evident, not just
each link.

The chain is **durable**: records are persisted append-only and survive a restart, so the
proof outlives the process that produced it. The signing key is loaded from the environment
(`AILU_ATTESTATION_KEY`, a base64 PKCS8 key) so verification holds across restarts and
across instances; a control plane falls back to an *ephemeral* key in development only, with a
loud warning — an ephemeral key makes prior attestations unverifiable after a reboot, so it is
never used in production (a KMS/Vault-held key is the hardening from there).

### Verifying the chain

A control plane exposes the run's chain — and verifies it for you server-side — at
`GET /runs/:runId/attestations`:

```json
{
  "records": [
    { "approvalId": "ap-1", "subject": "tool:refund", "status": "approved",
      "resolvedBy": "alice@acme.eu", "prevHash": null,        "payloadHash": "9f2c…", "signature": "…" },
    { "approvalId": "ap-2", "subject": "tool:payout", "status": "rejected",
      "resolvedBy": "bob@acme.eu",   "prevHash": "9f2c…",     "payloadHash": "41ab…", "signature": "…" }
  ],
  "verified": true
}
```

`verified` is the result of re-checking every signature **and** every `prevHash` link over the
whole run. An auditor doesn't take your word for it — they call the endpoint (or run the same
check offline against the records) and get a yes/no.

Combined with the [event journal](./observable-runs) — which captures the suspend, the pending
request, the resolution, and the resume in order — you get an audit trail that is both
**complete** (every transition is an event) and **tamper-evident** (every decision is signed
*and* chained).

### Proof, not just a signature

A signature says "a human authorised this." Because Ailu runs are
[deterministic and replayable](../core-concepts/execution-contract), the attestation points at
a run you can **re-derive from its checkpoints** — so the evidence is the decision *and the
exact execution that led to it*, not an assertion bolted on beside it. That is the difference
between a system that *attests and blocks* and one that *runs, resumes, and replays* the
governed work itself. [Replay as evidence](./replay-as-evidence) makes that concrete: it
re-derives the run and proves the replay reaches the **same decisions** — a faithfulness
guarantee distinct from the tamper-evidence the chain gives you.

## End to end

```text
agent → gated tool `refund`
   └─ run_suspended ─────────────▶ approval record { requestedBy: assistant, subject: tool:refund, pending }
                                      │
        human (≠ agent) approves  ────┤  resolvedBy = <principal>;  decision attested (Ed25519)
                                      ▼
        approveAndResume  ──▶ __approvedTools = ["refund"]  ──▶ agent re-runs, `refund` executes  ──▶ run_completed
```

## Next

- [Observable runs](./observable-runs) — surface all of this as a live view.
- [Approval gates](./approval-gates) — the builder-side API.
