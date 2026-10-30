---
title: "Upgrading to 2.8"
description: "2.8.1 descendant-approval recovery, complete approval-store evidence, trusted checkpoints, and 2.8 evaluator and cost migrations."
---

# Upgrading to 2.8

2.8.0 is a minor release. Nothing is removed and no signature changes. Two behaviours change for a
graph or a dashboard that relied on the old one: the `evaluator` component on a missing input, and
the `ailu.cost.usd` span attribute for an unknown model. Pin the exact version
(`@ailu-ai/graph-sdk@2.8.1` with the matching native addon) and read the sections below before upgrading.

The full list is in the [changelog](https://github.com/AI-Adriane/ailu-core/blob/main/CHANGELOG.md).

## Unreleased: globally unique catalog node IDs {#catalog-node-ids}

**Breaking behavior change:** catalog start, resume, and replay now reject a node ID repeated
within the root graph or across any supplied subgraphs, including nested and unused definitions.
The catalog returns `invalidDefinition` with `duplicate node id '<id>'`; the TypeScript catalog
helpers raise `TypeError`, and Python raises `RunError`. Validation happens before carrier
selection and host-binding resolution. Graphs with globally unique node IDs keep their behavior.

All catalog nodes share the same runtime registries. Previously, a child with the same ID could
replace the parent's carrier configuration or inherit a host binding. IDs are compared exactly
after JSON decoding: `"shared"` and `"\u0073hared"` collide. IDs are not trimmed, case-folded, or
Unicode-normalized.

Before starting a new run:

1. Inventory node IDs across the root graph and every supplied subgraph. Assign a distinct ID
   to every node, for example `parent-sub` and `child-sub` instead of reusing `sub`.
2. Update each renamed node's references, including `entryNodeId`, edge `from`/`to`, carrier
   references such as `mapAgents.joinAt`, and the host's node-binding IDs.
3. Rebuild the catalog with the matching SDK and native binary, and verify start, governed
   resume, and replay with the corrected definitions and newly created run state.

For an existing run whose definitions contain collisions, fence the run and reconcile prior
effects before starting a new run with corrected definitions. Renaming nodes changes execution
and approval identities: do not rewrite stored checkpoint, journal, or approval IDs to make an
old run pass validation.

## 2.8.1: descendant approvals and existing checkpoints {#checkpoint-recovery-281}

**2.8.1 intentionally blocks old checkpoints whose descendant gates were never filed.** This
security patch checks human gates in nested subgraphs and every `mapSubgraph` item. A governed
resume advances no gate while any required decision is missing, pending, rejected, self-approved,
or attached to a different run or node. The check also covers suspended children carried by a
cancelled or running root. A fully approved workflow continues normally.

An old run may have used the correct approval engine and still have no descendant requests:
the earlier walker did not discover them. `ApprovalNotGrantedError` (`AILU_APPROVAL_NOT_GRANTED`
in TypeScript) now explains that a descendant approval was never recorded. Retrying with the
same engine does not repair it. The state stays stopped for operator action. A malformed or
unsupported wait instead raises `ApprovalRefusedError`; do not migrate past that refusal.

### Recovering a trusted checkpoint

There is no automatic public re-file helper in `@ailu-ai/graph-sdk` 2.8.1. The internal
`engineApprovalPlan`, `engineResumeProblems`, and `engineApprovalsToCheck` wrappers are not
exports of its public entry point. Recovery is a host-owned migration using the existing
native approval planner; never reconstruct child identities or gate subjects by hand.
Install `@ailu-ai/napi@2.8.1` as a direct dependency of the migration tool, including when using
pnpm, and keep it at the same version as the SDK.

1. Fence the run against resume jobs and other writers. Back up its checkpoint, filed requests,
   and audit trail. Load its trusted, unchanged graph definition and all child definitions.
2. Make an in-memory copy of the saved checkpoint and set only that copy's
   `channels.__approvalIds` to `[]`. Omit `previousState`: this is planning a saved wait, not
   executing a resume. This is necessary for missing, partial, and stale stashes alike.
3. Ask the matching 2.8.1 native engine for the complete plan. The JavaScript entry is
   `@ailu-ai/napi.engineCatalogApprovalPlan` (JSON in, JSON out):

   ```ts
   import { createRequire } from "node:module";
   const { engineCatalogApprovalPlan } = createRequire(import.meta.url)("@ailu-ai/napi");

   // definition, subgraphs and checkpoint come from the fenced host's trusted storage.
   const planningState = {
     ...checkpoint,
     channels: { ...checkpoint.channels, __approvalIds: [] }
   };
   const plan = JSON.parse(engineCatalogApprovalPlan(JSON.stringify({
     graph: definition,
     subgraphs,
     state: planningState
   })));
   if (plan.refusal !== undefined) throw new Error(plan.refusal);
   // Planning executes no graph nodes. Do not persist planningState's empty stash.
   ```

   Rust hosts call `catalog_approvals::filing_plan`; C hosts use
   `ailu_catalog_approval_plan_json`. A Python-only host without a migration integration to
   the same planner should use the abandonment procedure below instead of guessing requests.
4. File **every** entry of `plan.requests` as a new pending request in the same tenant's approval
   store. Copy `runId`, `nodeId`, `requestedBy`, and `subject` exactly from the plan. Keep the
   old request records for audit, but do not copy their approved status or resolver into the
   replacement set. Show each new request to its human reviewer.
5. Durably replace the saved checkpoint's entire `__approvalIds` array with all returned ids,
   in plan order. Preserve all other state, including child snapshots and kept map results.
   Persist the complete request set and replacement checkpoint atomically, or with a host-owned
   idempotent migration record that can recover a crash while the run stays fenced. If that
   guarantee is unavailable, abandon the run; do not release an empty or partial stash.
6. After every new request is independently approved, release the migration fence and resume
   through the normal governed SDK entry with the same approval engine and definitions. Its
   normal pre-check must still authorize every descendant. A pending, rejected, unknown, or
   mismatched request keeps the run stopped. Do not call a raw runtime resume to skip that check.

Only migrate checkpoints from trusted engine storage. A refusal for missing definitions,
invalid indices, excessive depth or request count, or an uninterpretable snapshot is not
permission to edit the state until it passes. Reconcile the cause or abandon the run.

### Abandoning a legacy run

If recovery cannot be performed safely, mark the run abandoned in the host control plane and
stop further resume deliveries. Retain its checkpoint and audit records. Before starting a new
run, reconcile actions already completed: restarting from the entry can repeat side effects.
Never remove the approval engine, invent an approval, or roll back to a vulnerable engine to
make the old checkpoint advance.

### Third-party approval engines

`getById` must return the original `runId`, `nodeId`, `subject`, `requestedBy`, `status`, and,
for an approval, a nonempty independent `resolvedBy`. The Python equivalents are `run_id`,
`node_id`, `subject`, `requested_by`, `status`, and `resolved_by` from `get_by_id`. These identities
are part of the existing approval-record contract; 2.8.1 uses them to match every descendant.
An adapter that drops run or node identity now fails closed even when its record says approved.
Preserve the filed values in storage and on reads; do not synthesize them from the current run.

### Limits and rollback

The approval plan allows at most 16 descendant levels and 256 requests. It refuses the whole
wait if it cannot interpret it completely. The planned early size check at map entry is a
separate follow-up: an oversized map can execute actions **before** its gates before the plan
refuses it. Do not treat the filing ceiling as a pre-execution resource or side-effect limit.

Keep the SDK and native engine pinned together at 2.8.1. Returning to 2.8.0 or 2.7.x reopens the
descendant-gate bypass; the older rollback instructions below describe 2.8.0 feature compatibility,
not a safe rollback for this security fix.

## 2.8.1: catalog approval evidence {#unreleased-catalog-approval-evidence}

The 2.8.1 approval-ID replay fix changes the custom approval-store contract. TypeScript
`ApprovalEngine.getById` must return the stored `runId`, `nodeId`, `requestedBy`, full `subject`
object and `status`, plus a nonempty `resolvedBy` for an approved decision. Python `get_by_id`
returns `run_id`, `node_id`, `requested_by` and `resolved_by`, with the same `subject` and
`status`. The C JSON boundary uses the TypeScript field names. Preserve the complete subject
that was filed, including call input, call key, approval key and condition when present; a
description alone cannot replace a subject that contained call evidence. The built-in in-memory
stores already retain these fields; custom adapters must return them too.

**How to migrate.** Update the store and its read adapter before resuming existing runs. Backfill
missing fields only from the original trusted request/audit records, preserving the recorded
human decision. Never invent an approver, silently mark a request approved, or reconstruct
missing provenance from caller-supplied state. If the original evidence is unavailable, leave
the run suspended or abandon it and reconcile prior effects before starting a new run with new
requests and human decisions. Incomplete records now fail closed with `ApprovalNotGrantedError`.
This change adds no automatic high-level SDK recovery API. Do not roll back the engine to make
an incomplete record pass: that restores the approval-ID replay bypass.

**Trusted resume state.** The host must authenticate the caller, enforce tenant/run access, and
load the latest saved state from its own checkpoint store before governed resume. Do not accept
an arbitrary serialized `GraphState` or a caller-selected `runId` as authorization. The shared
check binds records to the run, node, requester and full subject; it does not bind them to a
particular checkpoint/version or consume the stored decision. Re-submitting an already resumed
snapshot, replacing its IDs with an earlier decision for the same gate/call, or reusing a run ID
therefore requires host-side protection. Fence concurrent resumes and atomically record which
checkpoint was resumed; a host action must also honor its `effectKey` for idempotency. Raw
runtime resume/update APIs and operation without an approval engine remain trusted host
capabilities. Forked checkpoints may carry the original stash; do not treat it as approval of
the fork. File requests and collect decisions for the fork's own run identities before governed
resume. A fork with stale IDs can fail closed and require host-managed recovery or abandonment.

## The evaluator says why it did not score

Before 2.8.0 a missing channel was read as an empty string. `exact`, `overlap` and `tokenF1` then
gave `1.0` on two missing channels, and an array or an object was flattened to JSON text and
scored. Now each input is classified, and a status other than `scored` gives a score of `0` and
`passInto: false`, whatever the threshold.

| Channel value | Class |
| --- | --- |
| absent, `null` | missing |
| string, number, boolean | text |
| an agent result (an object with a string `reasoning`) | text: its final answer |
| array, any other object | `invalidType` |

Statuses: `scored`, `missingExpected`, `missingActual`, `missingBoth`, `invalidType`. The type is
open (`#[non_exhaustive]` in Rust): handle an unknown status as « not qualified ». An engine
failure (`error`) fails the node and is never written to a channel.

Two optional params:

- `statusInto`: a channel that receives the status. It must be a non-blank name that differs from
  `into`, `passInto`, `expectedFrom` and `actualFrom`.
- `emptyIsMissing` (default `false`): a text that is empty after trimming counts as missing.

```ts
evaluator({
  expectedFrom: "reference",
  actualFrom: "answer",
  metric: "exact",
  into: "score",
  passInto: "passed",
  statusInto: "scoreStatus",
  emptyIsMissing: true
});
```

| `reference` | `answer` | `score` | `passed` | `scoreStatus` |
| --- | --- | --- | --- | --- |
| `"Paris"` | `"Paris"` | `1` | `true` | `scored` |
| `"Paris"` | absent | `0` | `false` | `missingActual` |
| absent | absent | `0` | `false` | `missingBoth` |
| `"Paris"` | `["Paris"]` | `0` | `false` | `invalidType` |
| `""` | `""` (with `emptyIsMissing: true`) | `0` | `false` | `missingBoth` |
| `""` | `""` (default) | `1` | `true` | `scored` |

**How to migrate.** Valid results are unchanged. Without `statusInto`, nothing new is written, so
existing graphs keep the same channels, but **a score of `0` is no longer proof of an
evaluation**: add `statusInto` and read the status before you trust a score, a pass rate or a
dashboard built on it. A graph that passed on two missing inputs now fails; that is the intended
fix, not a regression.

## Cost results say when a cost is unknown or estimated

TypeScript SDK only. `estimateCost(usage, model, priceBook?)` returns a `CostResult`; `amount` is
`null` when the cost is unknown, never `0`. `aggregateCosts` sums results and reports the
measured part with its `coverage`.

```ts
const book = { "acme-1": { inPerMtok: 3, outPerMtok: 15, source: "contract 2026" } };

estimateCost({ promptTokens: 1000, completionTokens: 500 }, "acme-1", book);
// { status: "known", amount: 0.0105, currency: "USD", coverage: 1,
//   unpricedTokens: 0, pricedTokens: 1500, reasons: [] }

estimateCost({ promptTokens: 1000, completionTokens: 500, cacheReadTokens: 200 }, "acme-1", book);
// { status: "estimated", amount: 0.0111, coverage: 1, reasons: ["cacheApproximated"], ... }

estimateCost({ promptTokens: 1000, completionTokens: 500 }, "no-such-model", book);
// { status: "unknown", amount: null, coverage: 0, unpricedTokens: 1500,
//   reasons: ["modelNotInBook"], ... }

estimateCost({ promptTokens: 1000 }, "acme-1", book);
// { status: "unknown", amount: null, reasons: ["invalidUsage"], ... }
```

| Status | Meaning |
| --- | --- |
| `known` | The model is in the price book and every token type has an explicit rate. |
| `estimated` | Priced, but from an indicative rate (every entry of the built-in book is) or with cache tokens counted approximately. |
| `unknown` | The model, the usage or the price is missing or invalid. |

**Changes to watch.**

- `ailu.cost.usd` is no longer emitted for an unknown model; it used to be `0`. An alert or a
  dashboard that sums it must read an absent value as « cost unknown », not as zero. Read
  `ailu.cost.status` (`known`, `estimated`, `unknown`) next to it.
- `promptTokens` and `completionTokens` are required. A usage without them, or with a count that is
  not a finite, non-negative number, is `unknown` (reason `invalidUsage`) instead of a cost with a
  `NaN` or negative amount.
- For an `estimated` result `ailu.cost.usd` can differ from before: cache-write tokens are
  counted, and cache reads that the provider counts inside `promptTokens` are billed once. Set
  `promptIncludesCacheReads` on a price entry to say which convention a provider uses.
- Spans are keyed by run and node, so concurrent child runs of a mapped subgraph each count, and
  the usage a subgraph or fan-out node forwards from its child runs is counted once, on the child.

## What ships together

| Piece | 2.8.0 |
| --- | --- |
| `@ailu-ai/graph-sdk` (TypeScript) | evaluator statuses, `approvalScope`, cost results |
| `@ailu-ai/napi` and its per-platform packages | the native engine the SDK loads: evaluator statuses and `approvalScope` live here |
| Python SDK | evaluator statuses and `approvalScope`; no cost API |
| C ABI | evaluator statuses (same engine) |
| `@ailu-ai/contracts`, `config`, `model-core`, `verify`, `ailu-cli` | version bump only, no API change |

Install the SDK and the native addon at the same version: the SDK depends on the matching
`@ailu-ai/napi`. Evaluator statuses come from the native component, so an SDK running on an
older addon is not a combination we test or support.

## Check a clean install

In an empty directory:

```bash
npm init -y
npm install @ailu-ai/graph-sdk@2.8.1
node -e "console.log(require('@ailu-ai/napi').engineVersion())"   # 2.8.1
```

Then run a graph with an `evaluator` that has `statusInto` and a missing `actualFrom`, and expect
`missingActual`; and call `estimateCost` on an unknown model and expect `amount: null`. Those two
checks fail on 2.7.x and pass on 2.8.0.

## Going back

**The steps below describe 2.8.0 feature compatibility only. Do not use them to recover a
2.8.1 governed run: older engines reopen the descendant-approval bypass.**

- **Pin back.** Change the pin to `@ailu-ai/graph-sdk@2.7.1` (and the Python SDK to its 2.7.1)
  and reinstall. No stored data changes format, so nothing has to be converted.
- **If a release must be withdrawn**, do not unpublish (npm refuses after 72 hours): move the
  `latest` dist-tag back to `2.7.1` and deprecate `2.8.0` with a message that points here; yank
  `2.8.0` on PyPI. Consumers pinned to an exact version are unaffected either way.
- After a rollback, the evaluator reads a missing input as an empty string again: re-check any
  threshold or pass rate that was tuned on 2.8.0 statuses.
