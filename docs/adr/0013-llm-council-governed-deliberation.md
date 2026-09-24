# ADR 0013 — LLM Council as a governed deliberation primitive

- Status: Proposed (pattern + value proposition; implementation is a follow-up)
- Date: 2026-06-22
- Deciders: Mathieu (owner)

## Context

Andrej Karpathy published **llm-council** (<https://github.com/karpathy/llm-council>): a small web
app that, for a single user query, (1) **dispatches** the question to several frontier models in
parallel, (2) has each model **peer-review and rank** the others' answers (responses anonymized so a
model can't favour its own), then (3) a designated **chairman** model **synthesizes** one final
answer from the ranked field. The thesis is simple and strong: for a non-trivial question, a panel
of independent models that critique each other beats any single model's one-shot answer — and the
ranking surfaces *why*.

As shipped it is a **bespoke script**: an ad-hoc orchestration with raw provider calls, no
persistence, no audit, no access control, no failure recovery. That is fine for a personal tool. It
is not fine for the use Ailu targets — **high-stakes, regulated decisions** (legal/contract
review, financial recommendation, medical/triage, incident severity calls) where you additionally
need: a record of *which model said what and who ranked whom*, the ability to **resume** a panel
that half-failed (a member timed out) without re-paying every other member, a **human override**
before the verdict is accepted, **PII redaction** on the input, and **provider/sovereignty**
freedom (mix Gemini + Anthropic + Mistral + a self-hosted sovereign model).

A council is, structurally, exactly the kind of multi-stage fan-out / fan-in graph Ailu already
runs. The question this ADR settles is whether to adopt it as a **first-class, reusable, governed
pattern** and, if so, how it executes.

## Decision

**Adopt the LLM Council as a governed deliberation primitive expressed as a native Ailu graph**,
and ship it as a reference pattern (a `benchmarks/business` / `examples` graph first, then a
`prebuilt.council(...)` once the shape stabilizes). Two firm design rules:

1. **Native Rust execution, no SDK "intelligence."** Every member, reviewer, and the chair is an
   `agentNode` that runs natively on the Rust engine and reaches its model through the
   `llm-gateway` connectors (Gemini/Anthropic/OpenAI-compatible/Ollama). **No raw `fetch` to a
   provider** in node handlers — that was the dogfooding mistake the benchmarks made; the council is
   the canonical example of doing it right (provider + model + native `system` prompt, the gateway
   resolves tenant-key-then-env). Pure-JS node handlers stay for *presentation/aggregation glue
   only* (anonymizing, shuffling, formatting the ranked field) — never for LLM calls.

2. **The graph is the deliberation; governance is the value.** The stages, the fan-out, the
   anonymized peer-review and the chair are the *easy* part. What makes it Ailu and not a script
   is everything the runtime gives any graph for free: determinism-with-checkpoints, the event
   audit trail, the optional human gate, PII redaction, RBAC and multi-tenant/sovereign execution.

### Graph shape

```
              ┌──────────────── dispatch (fan-out) ────────────────┐
   query ──▶  │  member_A   member_B   member_C   …   (agentNodes)  │
              └───────────────────────┬─────────────────────────────┘
                                      ▼
                          anonymize + shuffle (pure-JS)
                                      ▼
              ┌──────────── peer-review (fan-out) ─────────────┐
              │  reviewer_A   reviewer_B   …  rank the field   │   (agentNodes)
              └───────────────────────┬────────────────────────┘
                                      ▼
                          aggregate ranks (pure-JS, e.g. RRF/Borda)
                                      ▼
                          [optional human gate: accept/override]
                                      ▼
                          chair: synthesize final answer (agentNode)
```

- **dispatch** and **peer-review** are I/O fan-outs (N concurrent model calls). Per the
  `benchmarks/README.md` fan-out finding, the SDK builder does not yet expose the runtime's
  `fanOut`, so each fan-out is **one node that runs its members concurrently with an internal
  `Promise.all`** over `agentNode`-equivalent calls. (A `.fanOut()` builder method is the clean
  follow-up; until then this keeps the N calls parallel on the public SDK.)
- **anonymize/shuffle** and **aggregate** are deterministic pure-JS — no model, no governance
  surface, cheap.
- the **human gate** is optional and policy-driven (on for high-stakes namespaces).

## Value proposition

What a council buys you over a single model:

- **Higher-quality, calibrated answers** on hard questions — independent panels reduce single-model
  blind spots, and the peer ranking is a built-in confidence/consensus signal.
- **Explainability** — the ranked field shows *why* an answer won, not just the verdict.

What **Ailu specifically** adds over Karpathy's script (the reason to build it here):

| Capability | llm-council (script) | Council on Ailu |
| --- | --- | --- |
| Multi-model dispatch + peer-rank + chair | ✅ | ✅ (the graph above) |
| Parallel members | ✅ (async) | ✅ (internal `Promise.all` per fan-out node) |
| **Resume a half-failed panel** | ❌ re-run everything | ✅ checkpoint after every node — a timed-out member resumes without re-paying the others |
| **Audit trail** (who answered, who ranked whom, what the chair used) | ❌ | ✅ a node-lifecycle event per member/reviewer/chair → reconstructable + signable |
| **Human override** before the verdict lands | ❌ | ✅ optional human gate (`run_suspended` → approve/override) |
| **PII redaction** on input / hydrate on output | ❌ | ✅ per-namespace policy (ADR 0008) |
| **Provider/sovereignty mix** | hard-coded providers | ✅ any `llm-gateway` connector incl. self-hosted Ollama / sovereign endpoint |
| **RBAC / multi-tenant** | ❌ | ✅ runtime-native |
| Reusable, shippable primitive | bespoke | `prebuilt.council(...)` (planned) |

The headline: **the council is a *governed deliberation* primitive, not an ensemble trick.** When a
decision is high-stakes, you want multiple independent models, a transparent ranking, a synthesized
verdict, *and* an auditable record with a human override — Ailu makes that a first-class graph.

## Governance mapping

- **No-self-approval → anonymized peer review.** The same invariant that forbids an agent approving
  its own output maps onto the council: a member must not knowingly rank *its own* answer. Responses
  are anonymized + shuffled before peer-review, and the **chair is a distinct principal** from the
  members whose answers it weighs. (The chair synthesizes; it does not "approve" — a human gate is
  the approval seam when one is required.)
- **Audit.** Each member/reviewer/chair call is a node → the standard lifecycle events already give
  a per-model record; with the observability sink (ADR 0008 Part C) this is queryable and the
  attestation primitives can sign the panel outcome.
- **Resumability caveat (honest).** LLM calls are not deterministic, so a *re-generated* member
  answer would differ run-to-run. Ailu's guarantee is narrower and still valuable: it
  **checkpoints the produced outputs**, so resume replays committed member answers rather than
  re-calling the models — the panel is *resumable* even though generation isn't *reproducible*.

## Consequences

- **Cost & latency are inherent.** A council is ~N× the tokens of a single answer (members) plus the
  review + chair passes, and wall-time is bounded by the slowest member per fan-out. The pattern is
  for high-stakes questions, not the default path — document this so it is opt-in.
- **Motivates `.fanOut()`.** The council is the strongest concrete case for promoting the runtime's
  `executeFanOut` to a first-class SDK builder method; until then the internal-`Promise.all` node is
  the supported workaround.
- **Dogfooding contract.** Building the council *correctly* (agentNode + native connectors, no raw
  `fetch`) makes it the reference for how every business workflow should call models — and the
  cue to refactor the existing `benchmarks/business/*` demos onto the same path.
- **Follow-up, not a commitment to code today.** This ADR fixes the design + value story; the
  reference graph and the eventual `prebuilt.council(...)` are separate, reviewable work items.
