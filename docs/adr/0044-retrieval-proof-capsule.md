# ADR 0044 — Retrieval Proof Capsule: capture the exact set that influenced the model

- Status: **Accepted** (2026-08-11, Mathieu — "go ADR"). Open question #1 resolved per this ADR's
  own recommendation (a separate `retrievalCapsule` event, not an extension of the existing
  candidate-pool `retrievals` event). Implementation proceeds lot by lot (D1 → D2 → D3 → D4),
  engine first, product last — per this ADR's own scope rule.
- Date: 2026-08-11
- Deciders: Mathieu (owner)
- Follow-up to product issue #578 (Tranche of ailu#424), which documents the product-side
  symptom this ADR investigates and fixes at the source.

## Context

Verified directly against the code, not assumed.

**The product-side symptom is precise and already self-documented.** `run-evidence.ts` builds a
`proofCapsule` verdict that can never be anything but `missing`/`proof_candidate_pool_only`,
with `exactInfluence` hardcoded to `false` and this comment right above it:

```ts
// The current event contract contains only a pre-rerank candidate/full-corpus record. A future
// string value alone must never upgrade this claim without typed chunks, order, scores and hashes.
const exactInfluence = false;
```

That comment is the spec. This ADR's job is to make the engine emit exactly what it asks for:
typed chunks, final order, scores, and hashes — so the product side can compute `exactInfluence`
for real instead of hardcoding it.

**The retrieval pipeline is entirely engine-owned** (four stages, all in `ailu-engine`, none
in the product control plane) — traced through a real production graph
(`product/apps/api/src/brain/governed-ask-graph.ts`, the `/ask` flow):

1. `semanticRetriever` (`packages/graph-sdk/src/semantic-retriever.ts:118-120`) — vector leg,
   `k=8` in this graph (param default 4). `store.query(queryVector, k)` means the `VectorStore`
   itself only ever returns the top-`k` matches — anything beyond `k` is discarded even earlier
   than the handler, inside the store's own query, with no record kept anywhere.
2. `bm25Retriever` (`components.ts:1079`, `bm25RetrieverHandler`) — lexical leg, `k=8` in this
   graph (param default 4). Same shape (`{id, content, score}[]`), truncated to `k`. The
   `docs.length - k` lowest-scoring docs are dropped with no record kept anywhere.
3. `mergeRanker` (`components.ts:1381`, `mergeRankerHandler`) — Reciprocal Rank Fusion over the
   vector + lexical channels, `k=8`. `merged.slice(0, params.k)` (line ~1414) drops the RRF
   losers — again no record. Worse: line ~1417, `{ ...item, score }` **overwrites** whatever
   `score` the item already carried (the vector-similarity or BM25 score) with the fused RRF
   score. The prior score is gone, not just unused.
4. `reranker` (Rust, `crates/runtime-bridge/src/lib.rs:1676`, `build_reranker_node`, ADR 0060
   E1) — the SAME pattern: `map.insert("score", ...)` (line ~1732) overwrites the fused score
   with the cross-encoder's score. In `governed-ask-graph.ts` this node runs with
   `top_k = items.len()` (no truncation at this stage), so nothing is *discarded* here today —
   but the score-clobbering still destroys the fused score, and there is no algorithm/version
   tag recorded (which reranker backend, which model) even though `CrossEncoderReranker::from_env`
   (`crates/llm-gateway/src/cross_encoder.rs`) already knows exactly which endpoint/model it
   called.

**This is a systemic pattern, not four unrelated bugs**: every stage treats `score` as a single
mutable field it's free to overwrite, and every truncating stage (`semanticRetriever`,
`bm25Retriever`, `mergeRanker`) drops candidates with zero trace. There is no "candidate lineage"
concept anywhere in this pipeline for a capsule to be built from — it has to be added.

**A disconnected, unused second reranker exists and should not be confused with the real one.**
`crates/rag-pipeline/src/reranker.rs`'s `LlmReranker` sends `provider: "openai", model:
"mock-reranker"` — a placeholder, never wired to any real graph (`governed-ask-graph.ts` uses
the Rust `reranker` node kind, which dispatches to `CrossEncoderReranker`, not this). Confirmed
via `grep -rln CrossEncoderReranker` — only referenced from `runtime-bridge` and its own crate;
`rag-pipeline::retrieve_and_rerank` is dead/example code as far as the production `/ask` path is
concerned. Not in scope here; flagged so it isn't mistaken for the real path in review.

**Signing already exists on both sides of the repo split, each scoped to what it assembles** —
this ADR should not invent a third mechanism:

- Engine: `crates/approval-engine/src/attestation.rs`'s `Ed25519Attestor` — canonical JSON
  (recursively sorted keys) → SHA-256 → Ed25519 sign, chained via `prev_hash` so neither a field
  nor the ordering can change after the fact. Signs approval decisions, which the Rust runtime
  itself produces.
- Product: `product/apps/api/src/audit/audit-signer.ts`'s `AuditSigner`, keyed off
  `AILU_ATTESTATION_KEY` — signs run evidence/certificates, which the control plane itself
  assembles (`run-evidence.ts`).

The pattern in this repo is: **whoever assembles the record signs it.** The retrieval capsule's
raw facts (order, scores, chunks, algorithm/version) are assembled mid-run by the engine's own
graph nodes — but the *capsule* as a verifiable claim is assembled by `run-evidence.ts`
product-side, which already owns the `proofCapsule` verdict and already has `AuditSigner`. See
Decision D4 and the rejected alternative below for why this ADR proposes the engine emit
unsigned facts, not a signed artifact.

## Decision

### D1 — Additive provenance, never an overwritten `score`

Every stage in the pipeline (`bm25Retriever`, `semanticRetriever`'s vector leg, `mergeRanker`,
the `reranker` node) appends to a `provenance: ProvenanceStep[]` array on each item instead of
overwriting `score`. Each `ProvenanceStep` is `{ stage: string, algorithm: string, algorithmVersion:
string | null, score: number }` — e.g. `{ stage: "bm25", algorithm: "bm25", algorithmVersion:
"k1=1.2,b=0.75", score: 4.2 }`, then `{ stage: "rrf", algorithm: "rrf", algorithmVersion: "k=60",
score: 0.031 }`, then `{ stage: "rerank", algorithm: "cross-encoder", algorithmVersion:
"bge-reranker-v2-m3", score: 0.87 }`. `score` (top-level) stays as a convenience mirror of the
LAST step's score, for callers that don't care about lineage — but the full chain is always
there. This is the direct fix for "score initial" + "score rerank" + "algorithme/version" in
issue #578's capture list.

### D2 — Discarded candidates survive, with a reason, alongside the survivors

`bm25Retriever` and `mergeRanker` write BOTH `{into}` (the surviving `k`, unchanged contract) and
a new `{into}Discarded` channel: the dropped items, same shape, plus `{ discardedAt: string,
reason: string }` — e.g. `reason: "rrf_rank_below_k"`. `semanticRetriever` gets the same treatment
once `VectorStore.query` can return a candidate pool larger than `k` (open question #2). Purely
additive (new channel, existing `{into}` contract untouched) — no consumer of the existing
channels needs to change. This is the direct fix for "used vs discarded avec raison."

### D3 — A capsule-assembly point, at the node that actually builds the prompt

The exact set that influenced the model is whatever text actually got templated into the prompt
— not necessarily the full `reranked` channel (some other graph might slice `top-N` downstream;
`governed-ask-graph.ts` happens not to, but this must not be assumed generically). `promptBuilder`
(the component that already does this templating) gains an optional `capsule: { into: string,
chunksFrom: string }` param: when set, it writes a `RetrievalCapsule` to `{into}` —

```ts
type RetrievalCapsule = {
  order: string[]; // final chunk ids, in the exact order templated
  chunks: { id: string; provenance: ProvenanceStep[] }[]; // full lineage per surviving chunk
  discarded: { id: string; provenance: ProvenanceStep[]; discardedAt: string; reason: string }[];
  renderedHash: string; // sha256 of the EXACT rendered template substring sent to the LLM
  queryHash: string; // sha256 of the query text the retrieval stages actually ran against
};
```

`renderedHash` is computed over the actual interpolated string (post-`{{reranked}}`
substitution), not the raw channel value — the thing that matters for "did this exact text reach
the model" is what the model actually received.

### D4 — The engine emits the capsule unsigned; the product side signs it

Matches the existing division of responsibility (see Context). The control plane's run-event
persistence (already reading a candidate-pool retrieval event per issue #578's own "constat")
extends to also persist `RetrievalCapsule`. `run-evidence.ts` computes `exactInfluence` for real
(typed chunks + order + scores + hashes present and internally consistent) instead of the
hardcoded `false`, and the EXISTING `AuditSigner` signs the assembled evidence — no new signing
code, no new key material, no engine-side Ed25519 dependency added to `graph-sdk` or
`runtime-bridge`.

### D5 — Authorization/freshness

Out of scope for the engine change itself: `authorization`/`freshness` (issue #578's list) are
properties of the KNOWLEDGE SOURCE (which the product control plane manages — connectors,
namespaces, KB documents), not of the retrieval pipeline. The capsule's `chunks[].id` gives the
product side what it needs to look these up against its own source-of-truth at evidence-assembly
time; this ADR does not duplicate that data into the capsule.

## Consequences

- **Unblocks issue #578's own hardcoded gap directly** — `run-evidence.ts`'s `exactInfluence`
  can go from a permanent `false` to a real computed value once `RetrievalCapsule` exists and is
  persisted.
- **Purely additive to existing channel contracts** (D1's `score` mirror, D2's new
  `{into}Discarded` channel, D3's opt-in `capsule` param) — no existing graph, prompt, or test
  needs to change to keep working exactly as today. `governed-ask-graph.ts` and any other
  existing `/ask`-style graph opts in by adding the `capsule` param to its `promptBuilder` node.
- **New published-package surface**: `ProvenanceStep`/`RetrievalCapsule` types land in
  `@ailu/graph-sdk` (additive types, no breaking change to `Bm25RetrieverParams` /
  `MergeRankerParams` / the Rust reranker node's params) and get consumed product-side via
  `@ailu/contracts` if the capsule shape needs to cross the API boundary as a typed DTO
  (existing product convention — frozen published DTOs, extended via extra fields, never edited
  in place).
- **The Rust `reranker` node needs its own small change** (D1/D3's algorithm/version tag) even
  though it doesn't truncate — `CrossEncoderReranker` already knows its endpoint/model
  (`cross_encoder.rs`), just isn't asked to report it today.
- **Does not touch `rag-pipeline::LlmReranker`** — confirmed dead/example code on the production
  path; a separate cleanup (delete or wire it up) is out of scope here and not blocking.

## Rejected alternatives

- **Sign the capsule in the engine** (reusing `approval-engine`'s `Ed25519Attestor`, or
  extracting a shared `ailu-attestation` crate). Rejected: breaks the established "whoever
  assembles signs" pattern — the capsule as a *claim* is assembled by `run-evidence.ts`
  product-side (which decides what counts as "verified" for the whole run certificate, not just
  retrieval), and that side already owns the signing key + mechanism. Signing raw mid-run engine
  facts before the product side has even decided how they compose into the run's overall
  evidence would create two independent signatures for one claim, or force the engine to know
  about product-side verdict logic it has no business knowing about.
- **A single `score` field with a "was this rescored" boolean**, instead of the full
  `ProvenanceStep[]` chain. Rejected: issue #578 explicitly asks for BOTH "score initial" AND
  "score rerank" (plural, distinguishable) — a boolean can't reconstruct which score came from
  which stage once there are four stages, and a future fifth stage (e.g. a second-pass
  reranker) would need the same information again.
- **Fix `rag-pipeline::LlmReranker` too, since it's the same conceptual bug.** Rejected for this
  ADR's scope: it's not on the production `/ask` path (verified above), so fixing it doesn't move
  issue #578 forward and would be scope creep into unrelated dead code.

## Open questions

**#1 — Does the product-side `retrievals` event/channel get EXTENDED to carry `RetrievalCapsule`,
or does this need a NEW event/channel?** `run-evidence.ts` already parses something via
`RetrievalEvidenceSchema` for the existing candidate-pool-only record (`retrievals:
RetrievalRecord[]`). I have not read that schema's product-side definition closely enough to know
whether extending it in place is clean or whether the "pool" and "capsule" concepts are different
enough to warrant a second, separate event type. Leaning toward a separate `retrievalCapsule`
event (keeps the existing candidate-pool record's meaning stable, avoids retrofitting a schema
that's already relied upon) but want product-side confirmation before deciding.

**#2 — RESOLVED.** Checked `packages/graph-sdk/src/semantic-retriever.ts` directly: identical
shape to `bm25Retriever` (`{id, content, score}[]`, line 120's projection), `k`-truncated (line
102). The truncation happens even earlier than in `bm25Retriever` — `store.query(queryVector, k)`
(line 118) means the underlying `VectorStore` itself only ever returns the top-`k` matches;
anything beyond `k` never reaches this handler at all, let alone gets discarded with a reason.
D1/D2 apply here identically to `bm25Retriever` for the handler's own output, but D2's "discarded
with a reason" for the vector leg specifically requires `VectorStore.query` itself to return a
larger candidate set (or a count) than `k` so there's something to mark as discarded — a detail
this ADR should decide alongside D1/D2, not defer. Proposed: `VectorStore.query` gains an
optional `candidatePoolSize` (defaults to `k`, so zero behavior change unless a caller opts in)
that returns up to that many matches; `semanticRetriever` requests `max(k, candidatePoolSize ??
k)` and applies D2's discard-with-reason to the difference, same as `bm25Retriever`/
`mergeRanker`.
