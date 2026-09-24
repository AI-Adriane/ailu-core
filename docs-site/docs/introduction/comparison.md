---
sidebar_position: 2
title: How Ailu compares
description: An honest comparison vs LangGraph, Temporal, and Haystack — where Ailu fits and where it doesn't.
---

# How Ailu compares

Ailu is **early and narrow on purpose**. It does one thing the others don't make
first-class — **governance baked into a stateful, resumable agentic-graph runtime, behind a
single Rust engine with parity SDKs** — and it is honest about everything it does *not* yet do.

This page exists so you can rule Ailu *out* quickly if it's the wrong tool. The credibility
play here is candor, not a feature checklist that wins on paper.

:::warning Alpha software
Ailu is **0.1.0 (alpha)**. It is not battle-tested at scale. Several capabilities other
tools ship in production — clustering, durable timers, signals, a managed worker fleet — are
**not implemented** here. See the [roadmap](/docs/roadmap) for what's stable, experimental, and
reserved.
:::

## The honest one-liner

- **LangGraph** — the closest neighbour: stateful agent graphs with checkpointing and human-in-the-loop. Mature, large ecosystem, Python-first. Ailu's distinction is governance-as-runtime-property and the one-Rust-engine/polyglot design, not graph features.
- **Temporal.io** — a durable-execution platform. Far more mature and operationally proven for long-running, distributed workflows at scale. Not agent- or governance-shaped; you build those yourself on top.
- **Haystack** — an LLM/RAG framework with a deep integration catalog (providers, vector stores, retrievers, document stores). Ailu has a fraction of those integrations and is not trying to be a RAG framework.

## Feature matrix

Legend: ✅ first-class · 🟡 partial / early · ❌ not provided · — not the tool's purpose.

| Capability | Ailu (0.1.0) | LangGraph | Temporal.io | Haystack |
| --- | --- | --- | --- | --- |
| Deterministic-by-default execution (named-predicate routing, no `eval`) | ✅ | 🟡 (app-controlled) | ✅ (durable, deterministic workflows) | — |
| Checkpoint after **every** node + state mutation | ✅ | ✅ | ✅ (event-sourced history) | ❌ |
| Native governance: approval gates as a runtime property | ✅ | 🟡 (interrupt/HIL primitives, you build the policy) | 🟡 (signals, you build the policy) | ❌ |
| Separation of duties (agent never approves its own output), enforced | ✅ | ❌ (DIY) | ❌ (DIY) | ❌ |
| Attested decisions (Ed25519, tamper-evident audit) | ✅ | ❌ | ❌ | ❌ |
| One engine, many SDKs at parity for engine-owned functions | ✅ (TS + Python + C-ABI SDKs over one Rust core) | ❌ (Python-first; JS port is separate) | 🟡 (multi-SDK, but each is a client lib, not one core) | ❌ |
| Durable execution at scale (clustering, durable timers, signals, retries-as-platform) | ❌ | 🟡 (via LangGraph Platform / external infra) | ✅ (this is its core competency) | — |
| RAG / retriever / vector-store breadth | 🟡 (a RAG pipeline + a few pieces) | 🟡 | 🟡 | ✅ (deepest catalog) |
| Provider & tool integration breadth | 🟡 (small, curated) | ✅ (large) | — | ✅ (large) |
| Polyglot SDKs beyond TS/Python | 🟡 (C-ABI starter SDKs shipped; callback-heavy TS parity still in progress) | ❌ | ✅ (Go, Java, TS, Python, PHP, .NET shipped) | ❌ |
| Maturity / production track record | 🟡 alpha, pre-1.0 | ✅ widely deployed | ✅ very mature | ✅ mature |

A few entries deserve a footnote rather than a checkmark:

- **Determinism vs Temporal.** Temporal's determinism is the gold standard for *durable
  execution* (workflow code must be deterministic; the platform replays event history across
  process restarts and machines). Ailu's determinism is narrower: routing is fully
  inspectable (conditions are [named predicates, never `eval`'d](/docs/core-concepts/execution-contract)) and a run resumes exactly from its latest checkpoint — but Ailu has no cluster, no
  cross-host replay guarantees, and no durable timers. Don't read the matching ✅s as equivalence.
- **Governance in LangGraph/Temporal.** Both give you the *primitives* to pause and resume
  (interrupts, signals). Neither makes separation-of-duties, attribution, and attestation a
  property of the runtime — you assemble that yourself in application code. That assembly is
  exactly what Ailu provides out of the box. See [the governance model](/docs/governance/governance-model).

## Where the parity SDKs actually differ

The "one engine, many SDKs" row is genuine: the graph model, validator, DSL compiler, model
policy, catalogs, native component runs, and prebuilt-agent runs live **once in Rust** and the SDKs
call into that code. The C ABI now also exposes the callback runtime shape for custom nodes,
host tools, predicates, lifecycle events, resume, signals, and replay. Python is still the
asymmetric surface: its PyO3 binding remains JSON-in/JSON-out until it gets a callback runtime.
The full contract is in [one engine, many languages](/docs/sdk-parity/one-engine-two-languages).

## Benchmarks — speed & tokens

Measured against **LangGraph.js** and **Haystack**, same machine, warmed, sequential. The Ailu
engine runs on a Rust core; peers run on Node / Python. Reproduce with `benchmarks/run-all.sh` and
`benchmarks/run-compare.sh`. The takeaway is **ratios and scaling**, not absolute milliseconds.

### Orchestration overhead (no LLM)

Pure per-node engine cost — scheduling, channel update, checkpoint:

| framework | per node | runs/s | vs Ailu |
| --- | --: | --: | --: |
| **Ailu** (Rust) | **~44 µs** | 574 | 1.0× |
| Haystack (Python) | ~103 µs | 243 | 2.4× heavier |
| LangGraph.js (Node) | ~109 µs | 230 | 2.5× heavier |

Per-feature vs LangGraph.js: chain **2.4×** lighter, suspend/resume **2.4×**, conditional 1.7×.

### Real workflows are LLM-bound

On 8 business workflows (same prompts, same model on both engines), total wall-time is dominated by
the model — engine choice is **invisible end-to-end**. The fair measure is **engine overhead**
(total − LLM time): the orchestration cost the engine itself adds. Ailu is **1.2–4.2× lighter**
per workflow (e.g. support-triage 4.4 ms vs LangGraph 17.5 ms).

### Tokens: the engine is token-neutral

On identical prompts, Ailu and LangGraph consume the **same input tokens** (e.g. product-flow
1197 = 1197) — the graph is orchestration, it adds **zero token tax**. An Ailu graph reduced to a
single node matches a bare native call exactly (1-node 1874 ≈ native monolith 1950 tokens). Token
cost is a function of **prompts and model, never the engine**.

Ailu ships token-optimisation levers you flip on (env / per-stage): **terse output**, **inter-stage
context trimming**, **LLMLingua input compression**, **prompt caching**, and per-stage **model tiering**.
Measured single-lever best cases (Gemini): LLMLingua input **−50%**, context trim **−43% input**, terse
output **−32% output (−31% latency)**, caching **~−24% input cost** on a repeated prefix.

But — measured honestly across all 8 workflows with **everything on**, the levers are **lossy and not
free**. Total tokens fell **−19%**, yet an LLM judge found the optimised output **degraded on 5 of 8**:

| workflow | tokens saved | output quality |
| --- | --: | --- |
| finance-analysis | −18% | ★★★★★ nothing lost |
| code-migration | −30% | ★★★★ holds (end truncated) |
| rag-doc-qa | +2% | ★★★★ fine |
| observability | −19% | ★★★ lost impact/cost analysis |
| support-triage | −17% | ★★★ lost empathy/tone |
| contract-review | −28% | ★★ dropped a clause; maybe hallucinated numbers |
| incident-postmortem | −31% | ★★ **factual errors** (lossy compression corrupted the timeline) |
| product-flow (code) | −1% | ★★ code/UX scope lost |

So the honest rule: apply terse to **prose-summary** stages, **never** lossy compression on **fact /
legal / code** inputs, keep code verbatim, and validate quality per workflow. Efficiency is **opt-in, not
a free default** — you trade tokens for substance and must choose where that trade is safe.

> A 3-stage governed pipeline costs more tokens than a one-shot monolith (it re-sends context between
> stages) — the price of per-stage checkpointing, governance and resume, a choice you make, not engine
> overhead. Reduce to a single node and Ailu matches the bare call exactly.

## When to choose Ailu

Reach for Ailu when the **hard part of your problem is governance over an agentic graph**, and
you want that governance to be a guarantee of the runtime rather than something you re-implement
per workflow:

- You're running agents that touch real things (refunds, deploys, outbound mail) and you need a
  **human gate that suspends the run cleanly and resumes from the exact checkpoint** — with the
  approver provably different from the requester.
- You need a **tamper-evident audit trail**: who decided, what, when — attested with a signature,
  not a free-text log line. (See [tool approval and attestation](/docs/governance/tool-approval-and-attestation).)
- You want the same graph definition and validation behaviour **across TypeScript and Python**
  with no second implementation to drift.
- You value **inspectable, deterministic routing** (named predicates) over the flexibility of
  arbitrary code in your edges.
- You're comfortable on **alpha software** and want to shape it.

## When *not* to choose Ailu (yet)

Be honest with yourself here — picking the wrong tool wastes weeks:

- **You need durable execution at scale.** Long-running, distributed workflows with durable
  timers, signals, cross-host replay, and a managed worker fleet are **Temporal's** domain and
  are not implemented in Ailu. Don't try to make Ailu be Temporal.
- **You need a broad RAG / integration catalog today.** If your value is in the breadth of
  retrievers, vector stores, document stores, and provider connectors, **Haystack** (or
  LangChain) has years of integrations Ailu doesn't. Ailu ships a small, curated set.
- **You need a mature, production-proven graph framework right now.** **LangGraph** is widely
  deployed with a large community. Ailu is pre-1.0 and pre-scale.
- **You need fan-out parallelism or subgraph composition today.** These have schema slots but are
  [reserved, not implemented](/docs/roadmap). Don't build on them yet.
- **You need a polyglot SDK that isn't TypeScript or Python today.** Go/Java/PHP/.NET/Ruby and a
  native Rust SDK are *designed for* by the architecture but **not shipped**. If you need them
  now, the only multi-language option that exists is Temporal.

:::note The actual edge
Ailu's bet is narrow: **governance baked into a stateful agentic-graph runtime, plus a
one-Rust-engine / thin-binding design that makes polyglot parity tractable.** If that combination
isn't the crux of your problem, one of the more mature tools above will serve you better today —
and we'd rather you find that out on this page than three weeks in.
:::

## See also

- [Why Ailu](/docs/introduction/why-ailu) — the problem and mental model.
- [The governance model](/docs/governance/governance-model) — the differentiator, end to end.
- [Roadmap](/docs/roadmap) — feature status and what's coming.
