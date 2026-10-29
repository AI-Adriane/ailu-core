---
title: "Upgrading to 2.8"
description: "What changes in 2.8.0 for the evaluator and for cost results, how to migrate, which pieces ship together, and how to go back."
---

# Upgrading to 2.8

2.8.0 is a minor release. Nothing is removed and no signature changes. Two behaviours change for a
graph or a dashboard that relied on the old one: the `evaluator` component on a missing input, and
the `ailu.cost.usd` span attribute for an unknown model. Pin the exact version
(`@ailu-ai/graph-sdk@2.8.0`) and read the two sections below before upgrading.

The full list is in the [changelog](https://github.com/AI-Adriane/ailu-core/blob/main/CHANGELOG.md).

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
npm install @ailu-ai/graph-sdk@2.8.0
node -e "console.log(require('@ailu-ai/napi').engineVersion())"   # 2.8.0
```

Then run a graph with an `evaluator` that has `statusInto` and a missing `actualFrom`, and expect
`missingActual`; and call `estimateCost` on an unknown model and expect `amount: null`. Those two
checks fail on 2.7.x and pass on 2.8.0.

## Going back

- **Pin back.** Change the pin to `@ailu-ai/graph-sdk@2.7.1` (and the Python SDK to its 2.7.1)
  and reinstall. No stored data changes format, so nothing has to be converted.
- **If a release must be withdrawn**, do not unpublish (npm refuses after 72 hours): move the
  `latest` dist-tag back to `2.7.1` and deprecate `2.8.0` with a message that points here; yank
  `2.8.0` on PyPI. Consumers pinned to an exact version are unaffected either way.
- After a rollback, the evaluator reads a missing input as an empty string again: re-check any
  threshold or pass rate that was tuned on 2.8.0 statuses.
