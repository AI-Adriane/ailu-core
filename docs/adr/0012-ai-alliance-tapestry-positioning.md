# ADR 0012 — Positioning Ailu relative to the AI Alliance "Tapestry" project (gap analysis)

- Status: Accepted (strategic direction; no code commitment yet)
- Date: 2026-06-21
- Deciders: Mathieu (owner)

## Context

The AI Alliance launched **Project Tapestry** (<https://thealliance.ai/projects/tapestry>): an
open, globally **federated foundation-model training** platform. Its thesis — frontier models are
concentrated in a few orgs/regions; Tapestry lets many partners co-train a shared base model while
**keeping data local and sovereign** (only weight updates are shared, raw corpora never leave the
node), plus a **training-data catalog** described by rich metadata, and mission-locked consortium
governance. Roadmap: Phase 0 now (distributed-training framework + data-catalog prototype + cultural
realignment demo), Phase 1 (Sep 2026) N-node platform + governance entity, Phase 2 (EOY 2026) first
small base model from scratch + sovereign derivatives, Phases 3–4 (2027+) industry/government
deployment then frontier scale.

We assessed where Ailu stands relative to Tapestry and what would have to change to align.

## Decision

**Treat Tapestry as complementary infrastructure at a different layer, and position Ailu as the
sovereign agentic application + governance layer of the open AI stack.** Ailu does not compete
with Tapestry; it sits above it:

- **Tapestry = training layer** — produces sovereign foundation models + a sovereign data catalog.
- **Ailu = inference / orchestration / governance layer** — runs and governs agents (resumable
  graphs, human gates, RBAC, signed audit, AI-Act compliance) on top of models.

The natural relationship: **Tapestry trains the sovereign models; Ailu deploys and governs them
as agents.** Both share the "sovereignty + EU + open" thesis, so the alignment is strategic, not
incidental.

## Gap analysis

| Tapestry dimension | Ailu today | Gap / opportunity |
| --- | --- | --- |
| Sovereignty & data-locality | Sovereign modes (ADR 0006), self-host, EU hosting, per-namespace PII | ✅ Aligned. Ailu is the applied showcase of Tapestry-style sovereignty |
| Consuming sovereign models | `llm-gateway` + OpenAI-compatible / Ollama / custom-base-URL adapters | ✅ Near-zero gap — a self-hosted Tapestry derivative (OpenAI-compatible endpoint) already runs in Ailu |
| Data catalog + contribution metadata (provenance, license/restrictions, PII status, quality/AI-readiness, jurisdiction, permitted uses) | KB/OKF (frontmatter + knowledge graph) + **real PII policies** (Presidio/GLiNER) + connectors | ⚠️ **Real gap**: OKF lacks license / jurisdiction / permitted-uses / AI-readiness fields. Ailu is well-placed (already has provenance + PII status) → build a **Tapestry-catalog-conformant export** |
| Evaluation & preference data | Human approvals = preference signals; run traces = eval data; ed25519-signed attestations | ⚠️ **Gap**: evaluation is deferred (DTOs only); no export of preference/eval datasets. Strong opportunity — approval gates are already labelled preference |
| Federated multi-node training (weights-only) | Fleet/workers (multi-node **execution**), no training | ❌ Out of scope — a boundary, not a gap to close |
| Governance | Runtime: RBAC, no-self-approval, signed audit, AI-Act reports | Tapestry: mission-locked model/data governance → complementary. Ailu's attestation primitives could feed Tapestry "contribution accounting" |

## Prioritized gaps to plug into the Tapestry ecosystem

1. **P1 — Data-catalog export** (low cost, high leverage): extend OKF/KB metadata with
   license, jurisdiction, permitted-uses and AI-readiness, and emit a manifest conforming to the
   Tapestry data catalog. Ailu already carries provenance + PII status, so this is the cheapest
   path to becoming a *contributor*, not just a consumer.
2. **P2 — Preference/eval export**: turn approvals + run traces into contributable
   preference/evaluation datasets. Doubles as the long-deferred "evaluation" feature.
3. **P3 — Sovereign-model recipe** (nearly done): document/package running a self-hosted Tapestry
   derivative through the Ollama / OpenAI-compatible seam (the self-hosted-Ollama work already lays
   this groundwork).

## Consequences

- A clear, non-competing story: "the sovereign agentic runtime for open, sovereign models."
- Consider **joining the AI Alliance** — Ailu fills their application/orchestration/governance
  gap; Tapestry fills Ailu's sovereign-model gap.
- P1 + P2 are the concrete engineering bets that convert the positioning into a real two-way
  relationship (Ailu both consumes Tapestry models and contributes data + preference signals).
- No immediate code commitment; this ADR records the direction and the gap backlog.

## Alternatives considered

- **Ignore Tapestry** — misses an aligned, sovereignty-first ecosystem exactly where Ailu is
  strong.
- **Compete (build training/federation)** — wrong layer; Ailu is an orchestration/governance
  runtime, not a model-training platform.
