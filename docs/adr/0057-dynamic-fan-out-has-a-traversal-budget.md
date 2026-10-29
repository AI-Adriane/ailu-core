# ADR 0057 — Dynamic fan-out has a traversal budget

- Status: **Accepted design**, following the owner's
  [limit/configuration decision](https://github.com/AI-Adriane/ailu-core/pull/413#issuecomment-6497090434)
  and [design approval](https://github.com/AI-Adriane/ailu-core/pull/413#issuecomment-6497703287).
  Implementation readiness in PR #413 requires combined validation after #411 and #407;
  design approval does not itself authorize a merge or release.
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
first-come admission pool makes failed child indices depend on checkpoint/provider timing;
a global semaphore held by parent maps can deadlock while they await descendants.

## Decision

The owner selected configurable host/runtime limits with engine-enforced hard ceilings:

| Host option | Default | Hard ceiling | Meaning |
| --- | ---: | ---: | --- |
| `maxItems` | 128 | 1,024 | Maximum input array length for either dynamic map kind; reject, never truncate. |
| `maxConcurrency` | 8 | 32 | In-flight futures in one map, preserving input order. |
| `maxTotalItems` | 512 | 4,096 | Mapped items admitted across one traversal and its descendants. |
| Dynamic depth | 8 | 8, fixed | Maximum dynamic fan-out levels; ordinary subgraphs do not increment it. |

All configurable values are positive integers. Invalid values, including zero or values above
a ceiling, fail construction rather than being clamped. Omitted fields take their defaults.
Rust exposes the validated `FanOutLimits` type with private fields and a typed
`FanOutLimitsError`. The TypeScript and Python constructors validate and snapshot host settings
with `InvalidFanOutLimitsError`; Rust independently validates the wire settings too.

The item ceiling bounds per-map cloning/future allocation; bounded concurrency reduces
simultaneous IO; aggregate admission and dynamic depth prevent nested maps from resetting
limits. Raising a host limit does not remove its hard engine ceiling.

### Host boundary and APIs

Limits belong to trusted runtime options, never `GraphDefinition`, its metadata, a channel,
input data or a checkpoint. The host must keep the engine-spec envelope trusted rather than
merging arbitrary request JSON into its top-level fields. An untrusted graph/input can supply
keys named `fanOutLimits`, but they cannot affect admission or scheduling.

```rust
let limits = FanOutLimits::new(256, 4, 1024)?;
let runtime = GraphRuntime::new(graph, nodes, conditions).with_fan_out_limits(limits);
```

TypeScript builder compilation accepts `.compile({ fanOutLimits: { maxItems: 256 } })`;
`safeCompile` returns a typed configuration failure when options are invalid. Catalog
start/resume/replay take `fanOutLimits` as a host option. The option is copied separately from
the saved graph, and mutation of the caller's settings after construction does not affect a
runner. The default no-argument `safeCompile` keeps its existing error contract.

Python compilation accepts `.compile(fan_out_limits={"maxItems": 256})`.
`GraphRunner(..., fan_out_limits=...)` and the catalog start/resume/replay helpers accept the
same camelCase keys inside the mapping. Raw N-API, PyO3 and C JSON callers set only the trusted
top-level `EngineSpec.fanOutLimits`; no binding function signature changes.

### Deterministic accounting and executor context

After a map admits its array, divide the remaining allowance among concurrent children in
input order: each receives `remaining / count`, with the remainder assigned to the first items.
Static parallel branches also receive shares because their handlers can run dynamic maps.
Branches cannot borrow from siblings while the batch runs. Return unused shares after the
batch, without refunding admitted work. Four parent items leave 508 under defaults: each child
has 127, so a nested 128-item map fails in every branch regardless of polling order.

Keep accounting outside serialized state. The executor-independent scope is installed only
during future polling and restored afterward. Nested traversal inherits the limits and the
remaining allowance; independent roots stay isolated. A nested runtime cannot raise an
inherited limit by configuring a new root of its own.

Native handlers that spawn tasks must construct `with_fan_out_scope(child_future)` **before**
moving the future into `spawn` or `spawn_blocking`. The wrapper carries its captured scope
across threads. A task that first constructs the wrapper without that context starts a new
root; automatic executor propagation is not provided. Native handlers are trusted integration
code, not an untrusted sandbox.

An oversized array or exhausted allowance/depth permanently fails the affected map before
its children run. Enclosing maps keep their existing per-item error handling. Missing, null,
and empty input keep their no-op behavior. Cancellation remains cooperative at node boundaries;
it does not preempt an in-flight provider call or stop queued agent items mid-node.

## Compatibility and migration

Graphs exceeding the new defaults change behavior. The host can choose reviewed higher values
within the hard ceilings. Larger workloads must split inputs into separately scheduled runs,
batch before mapping, flatten nested maps, or reduce child batches to fit their allocated
shares. Several sequential maps still share one traversal allowance. Keep host rate,
concurrency, and spending controls when splitting work across runs. Callers must handle
permanent map failure and per-item errors instead of retrying unchanged oversized inputs.
Existing checkpoints with larger pending inputs need explicit host configuration/migration;
the engine never silently truncates them.

Limits and remaining allowance are not checkpointed. A retained runtime/compiled runner keeps
its host configuration, but catalog resume/replay must receive it again; omission uses safe
defaults. Every start/resume/replay gets a fresh traversal allowance. Completed subgraph items
remain reusable, although admission conservatively charges the full current array before
reuse is determined. Historical replay can fail under different limits or policy versions;
retain the originating engine version and host options where exact re-derivation is required.

Concurrency is per map; nested maps can exceed that count in aggregate. Admission counts
mapped items, not provider calls, agent iterations/retries, tokens, payload bytes, time, or
money. It is not a lifetime run quota, tenant quota, process-wide semaphore, or replacement
for approval checks. Raw single-child recursion is not governed by dynamic depth.

## Review and landing

[PR #407](https://github.com/AI-Adriane/ailu-core/pull/407) changes descendant approval filing,
and [PR #411](https://github.com/AI-Adriane/ailu-core/pull/411) protects approval identity and
subgraph mappings. Neither is implicitly merged or approved here. Land the PRs serially,
update each remaining branch from `main`, resolve overlapping paths, and rerun descendant
approval and full validation suites on the actual combined head before merging. An oversized
rejected map must run no host action and create no child wait or partial approval stash.
Temporary combined-tree tests are useful evidence, not substitutes for those merge gates.

## Validation

Regressions cover default 128 acceptance/129 rejection, configured admission/concurrency,
each hard ceiling and zero rejection, ignored graph/channel overrides, construction snapshots,
and option forwarding through native, TypeScript, and Python start/resume/replay paths.
Other tests cover bounded concurrency, mixed/recursive expansion, independent roots,
polling-order-independent admission, unused allowance, ordinary scope inheritance, explicit
spawned/blocking scope propagation, and no approval stash/host effect for rejected maps.
Existing checkpoint, approval, cancellation/resume, completed-item reuse and event tests remain
required alongside formatting, workspace Clippy and the Rust/SDK suites.
