---
sidebar_position: 4
title: Replay as evidence
description: Re-derive a governed run from its checkpoints and prove it reproduces the decisions it was attested for.
---

# Replay as evidence

[Attestation](./tool-approval-and-attestation#attestation) proves a decision **was not altered** —
tamper-evidence. It does not, on its own, prove the decision is the one the run *actually
produces*. A signed record could faithfully attest a decision that a re-run no longer reaches.

Because Ailu runs are [deterministic and replayable](../core-concepts/execution-contract),
Ailu closes that gap directly: it **re-derives the run from its checkpoints** and checks that the
replay reaches the **same governed decisions**. That is a second, independent guarantee.

| Guarantee | Question it answers | Mechanism |
| --- | --- | --- |
| **Tamper-evidence** | Were the attested records altered, dropped, or reordered? | Ed25519 hash-chain — [`verifyChain`](./tool-approval-and-attestation#verifying-the-chain) |
| **Faithfulness** | Does the run *reproduce* the decisions it was attested for? | Deterministic replay — `verifyReplayDecisions` |

You want both. A chain that verifies but whose decisions a replay can't reproduce is suspicious;
a faithful replay whose chain is broken has been tampered with. Together they say: *these are the
real decisions, and this is the exact execution that produced them.*

## How a replay stays deterministic

Re-running governed work would normally re-sample the LLM and re-read the wall clock — so two runs
would diverge and a replay would prove nothing. Ailu removes both sources of non-determinism:

- **Record** — running with `AILU_LLM_RECORD` set journals every agent's LLM input/output **and**
  the run's timestamp sequence into a compact `{ decisions, clock }` journal.
- **Replay** — re-feeding that journal makes each LLM call serve its recorded response (never a
  fresh sample) and each timestamp replay in order. Node ids are already sequence-based, not random.

A replay therefore **forks** a fresh, read-only run (`<runId>:fork:<n>`) from a checkpoint and
re-executes it identically. It never advances, resolves, or re-opens the original run — it opens no
new approval gate. It is evidence, not a second chance to decide.

## The SDK surface

Two functions in `@ailu/graph-sdk`:

```ts
import { runCatalogGraph, replayCatalogGraph, verifyReplayDecisions } from "@ailu/graph-sdk";

// 1. RECORD — capture the run's journal alongside its result.
//    (a control plane sets AILU_LLM_RECORD and persists outcome.replayJournal + the checkpoints)
const recorded = await runCatalogGraph(definition, { runId, initialData });
const journal = recorded.replayJournal; // `{ decisions, clock }` JSON

// 2. REPLAY — fork a deterministic re-execution from a persisted checkpoint, re-feeding the journal.
const replayed = await replayCatalogGraph(definition, checkpointState, checkpointId, journal);

// 3. VERIFY — the attested decisions must match the replayed ones, in order.
const attested = chain.records.map((r) => ({ status: r.status, subject: r.subject }));
const decisions = readDecisions(replayed.state); // { status, subject }[], derived the same way
const result = verifyReplayDecisions(attested, decisions);
// result.ok === true  →  the replay reproduced every decision in order
// result.mismatches   →  per-position divergences (flipped status, changed subject, dropped/added)
```

`verifyReplayDecisions` is pure and crypto-free: it compares the ordered `{ status, subject }` sets.
Order is significant — a dropped, reordered, or status-flipped decision is a mismatch. Wall-clock,
resolver identity, and approval ids are intentionally **not** compared: those are facts a
re-execution cannot (and should not) reproduce. `subject` is derived identically on both sides
(the decision's description, else the canonical JSON of its subject), so the comparison is exact.

:::note Deriving the checkpoint to replay from
To re-derive a *whole* run you replay from its **entry state** (the initial state, before the entry
node ran). A record-mode `runCatalogGraph` surfaces that state as `outcome.entryState` — the control
plane persists it (alongside the pure journal) and later feeds it back as `checkpointState` to
`replayCatalogGraph`. The replay re-derives deterministically and re-suspends at the first gate,
surfacing the requested subjects in `outcome.pendingApprovals` — the faithfulness signal compared to
the attested chain. The serving endpoint that performs record → replay → compare lives in the
control plane (Ailu Studio, or one you build).
:::

## Next

- [Tool approval and attestation](./tool-approval-and-attestation) — the tamper-evidence half.
- [Observable runs](./observable-runs) — the event journal that orders the suspend, resolve, resume.
- [Execution contract](../core-concepts/execution-contract) — why determinism makes replay possible.
