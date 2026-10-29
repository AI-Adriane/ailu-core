# ADR 0057 — Dynamic fan-out has a traversal budget

- Status: **Proposed** in [PR #413](https://github.com/AI-Adriane/ailu-core/pull/413); owner
  acceptance is required before merge. This records the candidate behavior, not an approval.
- Date: 2026-10-29
- Decider: repository owner
- Relates to: [0050](./0050-each-fan-out-spawn-reports-its-lifecycle.md) (map input and events),
  [0043](./0043-recursive-replay-for-subgraphs.md) (child identities and replay),
  [0049](./0049-the-host-keeps-each-checkpoint.md) (durable checkpoints).

## Context

Both dynamic map kinds accepted any length of array from their configured channel, cloned the
items, and polled every child concurrently. Each subgraph child has an independent traversal
step counter, so that counter cannot bound nested fan-out. A caller controlling the mapped
channel could multiply provider calls, memory allocations, and child checkpoints.

The shared admission boundary is `fan_out_items`, before its clone. Ordered scheduling must
keep result indices, child identities, approval suspension, and completed-item reuse. A shared
first-come admission pool makes the failed child indices depend on checkpoint or provider IO
timing; a global semaphore held by parent maps can deadlock while they await descendants.

## Proposed decision

Use fixed engine safety constants, shared by the runtime and native agent-map handler:

| Limit | Value | Meaning |
| --- | ---: | --- |
| Input items | 64 | Maximum length of one dynamic map array; reject, never truncate. |
| Concurrency | 8 | Maximum in-flight futures within one dynamic map, preserving input order. |
| Traversal admission | 256 | Mapped items admitted across the current traversal and its children. |
| Dynamic depth | 8 | Maximum dynamic fan-out levels; ordinary subgraphs do not increment it. |

These values put finite ceilings on common list workloads without serializing small maps.
The item ceiling bounds per-map cloning and future allocation; concurrency reduces simultaneous
IO; aggregate admission and dynamic depth prevent nested maps from resetting the limits.
The values are initial policy choices requiring owner acceptance, not workload-derived optima.

After a map admits its own array, divide the remaining allowance among concurrent children in
input order: each receives `remaining / count`, with the remainder assigned to the first items.
Static parallel branches also receive shares because their handlers can run dynamic maps.
Branches cannot borrow from siblings while the batch runs. Return unused shares after the
batch, without refunding admitted work. Four admitted parent items leave 252, so each child
has 63: a nested 64-item map fails in each branch regardless of polling order.

Keep accounting outside channels and serialized state. The executor-independent scope is
installed only during future polling and restored afterward. Nested traversal inherits it;
independent roots remain isolated. Native handlers that spawn tasks must construct
`with_fan_out_scope(child_future)` **before** moving that future into `spawn` or
`spawn_blocking`. The wrapper carries the captured scope across threads. A task that first
constructs the wrapper without that context starts a new root; automatic executor propagation
is not provided. Such native handlers are trusted integration code, not an untrusted sandbox.

An oversized array or exhausted allowance/depth permanently fails the affected map before its
children run. Existing enclosing-map per-item error handling still applies. Missing, null, and
empty input retain their no-op behavior. Node-boundary cancellation remains cooperative; the
engine does not preempt an in-flight provider call or stop queued agent items mid-node.

## Compatibility and migration

This changes public runtime behavior for graphs that previously mapped more than 64 items or
expanded beyond a branch's share. The engine does not add graph or SDK configuration fields.
Split larger inputs into separate bounded runs, or batch them before mapping. Merely dividing
an input into several sequential maps does not remove their shared traversal limit of 256.
Flatten nested maps, reduce per-child batches to fit their allocated shares, or move batches
to separate host-scheduled runs with the host's own rate, concurrency, and spending controls.
Callers must handle permanent map failure and per-item child errors rather than retrying the
same oversized input. Existing checkpoints with larger pending inputs will fail on resume
until the host explicitly migrates or replaces that work; do not silently truncate them.

The budget is not checkpointed: start, resume, and replay each begin a fresh traversal allowance.
Completed map-subgraph items remain reusable, although admission conservatively charges the
full current array before reuse is determined. A replay recorded before this behavior can fail
if its input exceeds the new limits. Do not assume byte-identical replay across engine policy
versions; retain the originating engine version when historical replay must match.

Eight is a per-map concurrency bound; nested maps can have more than eight active leaves.
Admission counts mapped items, not provider calls, agent iterations/retries, tokens, payload
bytes, time, or money. It is not a lifetime run quota, tenant quota, process-wide semaphore, or
replacement for approval checks. Raw single-child recursion is not governed by dynamic depth.

## Review and landing

The owner must accept or revise these fixed values and their compatibility impact before
merging the draft. Host-configurable limits with safe defaults and a hard ceiling are a future
option; this candidate deliberately exposes no configuration API or disable switch.

[PR #407](https://github.com/AI-Adriane/ailu-core/pull/407) changes descendant approval filing,
and [PR #411](https://github.com/AI-Adriane/ailu-core/pull/411) protects approval identity and
subgraph mappings. Neither is implicitly merged or approved by this proposal. Land the PRs
serially, updating each remaining branch from `main`, resolving overlapping paths, and rerunning
the descendant-approval and full validation suites on the actual combined head before merge.
An oversized rejected map must run no host action and create no child wait or partial approval
stash. A temporary combined-tree test is useful evidence but does not replace those merge gates.

## Validation

Regression coverage checks rejected and boundary arrays, bounded provider/child concurrency,
mixed and recursive expansion, independent roots, polling-order-independent admission, returned
unused allowance, ordinary scope inheritance, and explicit scope propagation into spawned and
blocking tasks. The bridge checks that a rejected map files no approvals or host effects.
Existing checkpoint, approval, cancellation/resume, completed-item reuse, and event tests remain
required alongside formatting, workspace Clippy, and the Rust and SDK test suites.
